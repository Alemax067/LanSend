<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { getCurrentWebview } from "@tauri-apps/api/webview";

type PeerStatus = "unknown" | "online" | "offline";
type AddressType = "ipv4" | "ipv6";

const MAX_CLIPBOARD_TEXT_BYTES = 256 * 1024;

interface SystemInfo {
  os: string;
  arch: string;
  app_version: string;
}

interface Peer {
  id: string;
  peer_id: string | null;
  alias: string | null;
  address_type: AddressType;
  host: string;
  port: number;
  status: PeerStatus;
  last_seen: string | null;
  system_info: SystemInfo | null;
}

interface AppConfig {
  device_id: string;
  alias: string;
  listen_port: number;
  save_dir: string;
  refresh_interval_seconds: number;
  protocol_version: number;
}

interface LocalAddresses {
  ipv4: string | null;
  ipv6: string | null;
  ipv6_status: string;
}

interface TransferFileMeta {
  index: number;
  name: string;
  size: number;
}

interface ClipboardTextPayload {
  text: string;
  size: number;
}

interface TransferOffer {
  transfer_id: string;
  sender_id: string;
  sender_alias: string;
  files: TransferFileMeta[];
  clipboard_text: ClipboardTextPayload | null;
  total_size: number;
}

interface TransferOfferResponse {
  transfer_id: string | null;
  accepted: boolean;
  upload_token: string | null;
  expires_in: number | null;
  reason: string | null;
}

interface SelectedFile {
  kind: "file" | "clipboard";
  path: string;
  name: string;
  size: number;
  text?: string;
}

interface UploadResult {
  ok: boolean;
  saved_name: string;
  size: number;
}

const localInfo = reactive({
  ipv6: "Detecting",
  ipv4: "Detecting",
  alias: "Detecting",
});

const settings = reactive({
  alias: "",
  port: 38987,
  savePath: "",
  refreshIntervalSeconds: 60,
});

const loadError = ref("");

const peers = ref<Peer[]>([]);
const isRefreshingPeers = ref(false);
const isSettingsOpen = ref(false);
const isAddDeviceOpen = ref(false);
const editingPeerId = ref<string | null>(null);
const droppedFiles = ref<SelectedFile[]>([]);
const peerMessage = ref("");
const toastMessage = ref("");
const pendingOffer = ref<TransferOffer | null>(null);
const pendingDeletePeer = ref<Peer | null>(null);
const pendingSendPeer = ref<Peer | null>(null);
const isSendingOffer = ref(false);
const isManualRefreshingPeers = ref(false);
let probeTimer: number | undefined;
let toastTimer: number | undefined;
let unlistenTransferOffer: UnlistenFn | undefined;
let unlistenDragDrop: UnlistenFn | undefined;

const newPeer = reactive({
  addressType: "ipv4" as AddressType,
  host: "",
  port: 38987,
});

const totalFileSize = computed(() =>
  droppedFiles.value.reduce((total, file) => total + file.size, 0),
);
const selectedClipboard = computed(() => droppedFiles.value.find((item) => item.kind === "clipboard"));
const selectedFileCount = computed(() => droppedFiles.value.filter((item) => item.kind === "file").length);
const hasClipboardSelection = computed(() => Boolean(selectedClipboard.value));
const selectionSummary = computed(() => {
  const clipboard = selectedClipboard.value;
  if (clipboard) {
    return `Clipboard Text · ${clipboard.text?.length ?? 0} characters · ${formatBytes(clipboard.size)}`;
  }

  return `${droppedFiles.value.length} file${droppedFiles.value.length === 1 ? "" : "s"} · ${formatBytes(totalFileSize.value)}`;
});

function showToast(message: string) {
  toastMessage.value = message;
  if (toastTimer) {
    window.clearTimeout(toastTimer);
    toastTimer = undefined;
  }
  toastTimer = window.setTimeout(() => {
    toastMessage.value = "";
    toastTimer = undefined;
  }, 3200);
}

function formatBytes(size: number) {
  if (size === 0) return "0 B";

  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(size) / Math.log(1024)), units.length - 1);
  const value = size / 1024 ** index;

  return `${value.toFixed(value >= 10 || index === 0 ? 0 : 1)} ${units[index]}`;
}

async function handleDrop(event: DragEvent) {
  const files = Array.from(event.dataTransfer?.files ?? []);
  if (files.length > 0) {
    droppedFiles.value = await Promise.all(
      files.map(async (file) => ({
        kind: "file",
        path: "",
        name: file.name,
        size: file.size,
      })),
    );
    showToast("Use system file drag-and-drop so LanSend can access the file path.");
  }
}

async function applyDroppedPaths(paths: string[]) {
  try {
    const files = await invoke<Omit<SelectedFile, "kind">[]>("inspect_files", { paths });
    droppedFiles.value = files.map((file) => ({ ...file, kind: "file" }));
    toastMessage.value = "";
  } catch (error) {
    showToast(`Failed to read file info: ${String(error)}`);
  }
}

function clearFiles() {
  droppedFiles.value = [];
  toastMessage.value = "";
}

function removeDroppedFile(index: number) {
  droppedFiles.value = droppedFiles.value.filter((_, fileIndex) => fileIndex !== index);
  if (droppedFiles.value.length === 0) {
    toastMessage.value = "";
  }
}

async function pasteClipboardText() {
  try {
    const text = await readText();
    const size = new TextEncoder().encode(text).length;
    if (text.trim().length === 0) {
      showToast("Clipboard does not contain text.");
      return;
    }
    if (size > MAX_CLIPBOARD_TEXT_BYTES) {
      showToast(`Clipboard text is too large. Limit: ${formatBytes(MAX_CLIPBOARD_TEXT_BYTES)}.`);
      return;
    }

    droppedFiles.value = [
      {
        kind: "clipboard",
        path: "clipboard:text",
        name: "Clipboard Text",
        size,
        text,
      },
    ];
    toastMessage.value = "";
  } catch (error) {
    showToast(`Failed to read clipboard: ${String(error)}`);
  }
}

function transferFilesMeta(): TransferFileMeta[] {
  return droppedFiles.value
    .filter((file) => file.kind === "file")
    .map((file, index) => ({
      index,
      name: file.name,
      size: file.size,
    }));
}

function clipboardPayload(): ClipboardTextPayload | null {
  const clipboard = selectedClipboard.value;
  if (!clipboard?.text) return null;

  return {
    text: clipboard.text,
    size: clipboard.size,
  };
}

function selectedFilesRequest() {
  return droppedFiles.value
    .filter((file) => file.kind === "file")
    .map((file) => ({
      path: file.path,
      name: file.name,
      size: file.size,
    }));
}

async function loadLocalState() {
  try {
    const [config, addresses, savedPeers] = await Promise.all([
      invoke<AppConfig>("get_app_config"),
      invoke<LocalAddresses>("get_local_addresses"),
      invoke<Peer[]>("list_peers"),
    ]);

    applyConfig(config);
    applyAddresses(addresses);
    peers.value = savedPeers;
    newPeer.port = config.listen_port;
    loadError.value = "";
  } catch (error) {
    loadError.value = String(error);
  }
}

function applyConfig(config: AppConfig) {
  localInfo.alias = config.alias;
  settings.alias = config.alias;
  settings.port = config.listen_port;
  settings.savePath = config.save_dir;
  settings.refreshIntervalSeconds = config.refresh_interval_seconds;
}

function applyAddresses(addresses: LocalAddresses) {
  localInfo.ipv4 = addresses.ipv4 ?? "No IPv4 available";
  localInfo.ipv6 = addresses.ipv6 ?? "No IPv6 available";

  if (addresses.ipv6_status === "link_local_only") {
    localInfo.ipv6 = `${localInfo.ipv6} (link-local only)`;
  }
}

async function saveSettings() {
  try {
    const config = await invoke<AppConfig>("update_app_config", {
      update: {
        alias: settings.alias,
        listen_port: settings.port,
        save_dir: settings.savePath,
        refresh_interval_seconds: settings.refreshIntervalSeconds,
      },
    });
    applyConfig(config);
    resetProbeTimer();
    loadError.value = "";
    isSettingsOpen.value = false;
  } catch (error) {
    loadError.value = String(error);
  }
}

async function refreshPeers(manual = true) {
  if (isRefreshingPeers.value) return;

  isRefreshingPeers.value = true;
  isManualRefreshingPeers.value = manual;
  try {
    peers.value = await invoke<Peer[]>("probe_all_peers");
    if (manual) {
      showToast("Device status refreshed");
    }
  } catch (error) {
    loadError.value = String(error);
  } finally {
    isRefreshingPeers.value = false;
    isManualRefreshingPeers.value = false;
  }
}

function resetProbeTimer() {
  if (probeTimer) {
    window.clearInterval(probeTimer);
  }
  probeTimer = window.setInterval(() => refreshPeers(false), settings.refreshIntervalSeconds * 1000);
}

async function chooseSaveFolder() {
  const selected = await open({ directory: true, multiple: false });
  if (typeof selected === "string") {
    settings.savePath = selected;
  }
}

async function testNewPeer() {
  peerMessage.value = "Testing connection...";
  try {
    const peer = await invoke<Peer>("test_peer", {
      request: peerRequest(),
    });
    peerMessage.value = `Connected: ${displayPeerAlias(peer)}`;
  } catch (error) {
    peerMessage.value = `Connection failed: ${String(error)}`;
  }
}

function requestDeletePeer(peer: Peer) {
  pendingDeletePeer.value = peer;
}

async function confirmDeletePeer() {
  if (!pendingDeletePeer.value) return;

  try {
    peers.value = await invoke<Peer[]>("remove_peer", { id: pendingDeletePeer.value.id });
    showToast("Device deleted");
  } catch (error) {
    showToast(`Delete failed: ${String(error)}`);
  } finally {
    pendingDeletePeer.value = null;
  }
}

async function savePeerDialog() {
  peerMessage.value = "Saving device...";
  try {
    if (editingPeerId.value) {
      const peer = await invoke<Peer>("update_peer", {
        id: editingPeerId.value,
        request: peerRequest(),
      });
      peers.value = peers.value.map((item) => (item.id === peer.id ? peer : item));
      showToast("Device updated");
    } else {
      const peer = await invoke<Peer>("add_peer", {
        request: peerRequest(),
      });
      peers.value = [...peers.value, peer];
      showToast("Device added");
    }
    closePeerDialog();
  } catch (error) {
    peerMessage.value = `Save failed: ${String(error)}`;
  }
}

function openAddPeerDialog() {
  editingPeerId.value = null;
  newPeer.addressType = "ipv4";
  newPeer.host = "";
  newPeer.port = settings.port;
  peerMessage.value = "";
  isAddDeviceOpen.value = true;
}

function openEditPeerDialog(peer: Peer) {
  editingPeerId.value = peer.id;
  newPeer.addressType = peer.address_type;
  newPeer.host = peer.host;
  newPeer.port = peer.port;
  peerMessage.value = "";
  isAddDeviceOpen.value = true;
}

function closePeerDialog() {
  isAddDeviceOpen.value = false;
  editingPeerId.value = null;
  newPeer.host = "";
}

function peerRequest() {
  return {
    address_type: newPeer.addressType,
    host: newPeer.host,
    port: newPeer.port,
  };
}

function displayPeerAlias(peer: Peer) {
  return peer.alias ?? peer.host;
}

function peerAddress(peer: Peer) {
  return peer.address_type === "ipv6" ? `[${peer.host}]:${peer.port}` : `${peer.host}:${peer.port}`;
}

function handlePeerClick(peer: Peer) {
  if (droppedFiles.value.length === 0) {
    openEditPeerDialog(peer);
    return;
  }
  pendingSendPeer.value = peer;
}

function closeSendDialog() {
  if (!isSendingOffer.value) {
    pendingSendPeer.value = null;
  }
}

async function sendOfferToPeer(peer: Peer) {
  if (peer.status !== "online") {
    showToast("You can only send to online devices.");
    return;
  }

  isSendingOffer.value = true;
  showToast("Waiting for receiver approval...");
  try {
    const response = await invoke<TransferOfferResponse>("send_transfer_offer", {
      host: peer.host,
      port: peer.port,
      addressType: peer.address_type,
      files: transferFilesMeta(),
      clipboardText: clipboardPayload(),
    });
    if (!response.accepted) {
      showToast(`Receiver declined: ${response.reason ?? "rejected"}`);
      return;
    }

    if (hasClipboardSelection.value && selectedFileCount.value === 0) {
      showToast("Clipboard text sent.");
      pendingSendPeer.value = null;
      clearFiles();
      return;
    }

    if (!response.transfer_id || !response.upload_token) {
      showToast("Receiver approved, but the upload token is missing.");
      return;
    }

    showToast("Receiver approved. Uploading files...");
    const results = await invoke<UploadResult[]>("upload_transfer_files", {
      host: peer.host,
      port: peer.port,
      addressType: peer.address_type,
      transferId: response.transfer_id,
      token: response.upload_token,
      files: selectedFilesRequest(),
    });
    showToast(`Transfer complete: ${results.length} file${results.length === 1 ? "" : "s"} saved.`);
    pendingSendPeer.value = null;
    clearFiles();
  } catch (error) {
    showToast(`Send failed: ${String(error)}`);
  } finally {
    isSendingOffer.value = false;
  }
}

async function copyIncomingClipboardText() {
  if (!pendingOffer.value?.clipboard_text) return;

  try {
    await writeText(pendingOffer.value.clipboard_text.text);
    await decideIncomingOffer(true, "Copied clipboard text.");
  } catch (error) {
    showToast(`Failed to copy clipboard text: ${String(error)}`);
  }
}

async function decideIncomingOffer(accepted: boolean, successMessage?: string) {
  if (!pendingOffer.value) return;

  const transferId = pendingOffer.value.transfer_id;
  try {
    await invoke("decide_transfer", {
      decision: {
        transfer_id: transferId,
        accepted,
      },
    });
    showToast(successMessage ?? (accepted ? "Transfer request accepted. Waiting for upload." : "Transfer request declined."));
  } catch (error) {
    showToast(`Failed to handle transfer request: ${String(error)}`);
  } finally {
    pendingOffer.value = null;
  }
}

function scrollHorizontally(event: WheelEvent) {
  const target = event.currentTarget;
  if (!(target instanceof HTMLElement)) return;
  const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
  if (delta === 0) return;
  target.scrollLeft += delta;
}

function statusLabel(status: PeerStatus) {
  return {
    unknown: "Unknown",
    online: "Online",
    offline: "Offline",
  }[status];
}

onMounted(async () => {
  loadLocalState();
  unlistenTransferOffer = await listen<TransferOffer>("transfer-offer", (event) => {
    pendingOffer.value = event.payload;
  });
  unlistenDragDrop = await getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload.type === "drop") {
      applyDroppedPaths(event.payload.paths);
    }
  });
  resetProbeTimer();
});

onUnmounted(() => {
  if (probeTimer) {
    window.clearInterval(probeTimer);
  }
  if (toastTimer) {
    window.clearTimeout(toastTimer);
  }
  if (unlistenTransferOffer) {
    unlistenTransferOffer();
  }
  if (unlistenDragDrop) {
    unlistenDragDrop();
  }
});
</script>

<template>
  <main class="app-shell" @contextmenu.prevent>
    <div class="ambient ambient-one"></div>
    <div class="ambient ambient-two"></div>

    <section class="window-panel">
      <header class="top-bar">
        <div class="brand-block">
          <div class="brand-mark">LS</div>
          <div>
            <p class="eyebrow title-only">LanSend</p>
          </div>
        </div>

        <div class="status-strip" aria-label="Local status">
          <article class="status-chip wide">
            <span>IPv6</span>
            <strong>{{ localInfo.ipv6 }}</strong>
          </article>
          <article class="status-chip">
            <span>IPv4</span>
            <strong>{{ localInfo.ipv4 }}</strong>
          </article>
          <article class="status-chip alias-chip">
            <span>Alias</span>
            <strong>{{ localInfo.alias }}</strong>
          </article>
        </div>

        <button class="icon-button" type="button" @click="isSettingsOpen = true">
          Settings
        </button>
      </header>

      <p v-if="loadError" class="error-banner">{{ loadError }}</p>

      <section
        class="drop-zone"
        :class="{ 'has-files': droppedFiles.length > 0 }"
        @dragover.prevent
        @drop.prevent="handleDrop"
      >
        <div class="drop-orbit" aria-hidden="true">
          <div class="orbit-ring ring-one"></div>
          <div class="orbit-ring ring-two"></div>
          <div class="drop-core">⇪</div>
        </div>

        <div class="drop-actions">
          <button class="drop-clear-button" type="button" @click="pasteClipboardText">
            Paste Clipboard
          </button>
          <button v-if="droppedFiles.length > 0" class="drop-clear-button" type="button" @click="clearFiles">
            Clear
          </button>
        </div>

        <div class="drop-copy">
          <p class="eyebrow title-only">Drop Zone</p>
          <p v-if="droppedFiles.length > 0" class="drop-summary">
            {{ selectionSummary }}
          </p>
        </div>

        <div v-if="droppedFiles.length > 0" class="file-stack">
          <div
            v-for="(file, index) in droppedFiles"
            :key="file.path || file.name"
            :class="['file-pill', 'selected-file-pill', { 'clipboard-pill': file.kind === 'clipboard' }]"
          >
            <button class="file-remove-button" type="button" aria-label="Remove file" @click="removeDroppedFile(index)">×</button>
            <span>{{ file.kind === "clipboard" ? "Clipboard Text" : file.name }}</span>
            <strong>{{ formatBytes(file.size) }}</strong>
          </div>
        </div>
      </section>

      <footer class="device-dock">
        <div class="dock-heading">
          <div>
            <p class="eyebrow title-only">Saved Devices</p>
          </div>
          <button class="refresh-button" type="button" :disabled="isRefreshingPeers" @click="refreshPeers()">
            {{ isManualRefreshingPeers ? "Refreshing" : "Refresh" }}
          </button>
        </div>

        <div class="device-grid" @wheel.prevent="scrollHorizontally">
          <p v-if="peers.length === 0" class="empty-devices">No devices yet. Add one from the bottom-right button.</p>
          <button
            v-for="peer in peers"
            :key="peer.id"
            class="device-card"
            type="button"
            :disabled="isSendingOffer"
            @click="handlePeerClick(peer)"
            @contextmenu.stop.prevent="requestDeletePeer(peer)"
          >
            <span class="avatar">
              <span>{{ displayPeerAlias(peer).slice(0, 1).toUpperCase() }}</span>
              <i :class="['status-dot', peer.status]" :title="statusLabel(peer.status)"></i>
            </span>
            <strong>{{ displayPeerAlias(peer) }}</strong>
            <small>{{ peerAddress(peer) }}</small>
          </button>
        </div>
      </footer>
    </section>

    <Transition name="toast">
      <div v-if="toastMessage" class="toast-bubble" role="status" aria-live="polite">
        {{ toastMessage }}
      </div>
    </Transition>

    <button class="add-device-button" type="button" @click="openAddPeerDialog">
      <span>＋</span>
      Add Device
    </button>

    <div v-if="pendingSendPeer" class="dialog-backdrop" @click.self="closeSendDialog">
      <section class="settings-dialog" role="dialog" aria-modal="true" aria-label="Confirm send">
        <div class="dialog-title">
          <div>
            <p class="eyebrow title-only">{{ hasClipboardSelection ? "Send Clipboard" : "Send Files" }}</p>
          </div>
          <button class="close-button" type="button" :disabled="isSendingOffer" @click="closeSendDialog">×</button>
        </div>

        <p class="incoming-summary">
          Send {{ selectionSummary }} to {{ displayPeerAlias(pendingSendPeer) }}?
        </p>

        <pre v-if="selectedClipboard" class="clipboard-preview">{{ selectedClipboard.text }}</pre>
        <div v-else class="incoming-files send-files-preview">
          <div v-for="file in droppedFiles.slice(0, 5)" :key="file.path || file.name" class="file-pill">
            <span>{{ file.name }}</span>
            <strong>{{ formatBytes(file.size) }}</strong>
          </div>
        </div>

        <div class="dialog-actions">
          <button class="ghost-button" type="button" :disabled="isSendingOffer" @click="closeSendDialog">Cancel</button>
          <button class="primary-button" type="button" :disabled="isSendingOffer" @click="sendOfferToPeer(pendingSendPeer)">
            {{ isSendingOffer ? "Sending" : "Send" }}
          </button>
        </div>
      </section>
    </div>

    <div v-if="pendingDeletePeer" class="dialog-backdrop">
      <section class="settings-dialog" role="dialog" aria-modal="true" aria-label="Delete device">
        <div class="dialog-title">
          <div>
            <p class="eyebrow title-only">Delete Device</p>
          </div>
        </div>

        <p class="incoming-summary">
          Delete {{ displayPeerAlias(pendingDeletePeer) }} from saved devices?
        </p>

        <div class="dialog-actions">
          <button class="ghost-button" type="button" @click="pendingDeletePeer = null">Cancel</button>
          <button class="primary-button danger-button" type="button" @click="confirmDeletePeer">Delete</button>
        </div>
      </section>
    </div>

    <div v-if="pendingOffer" class="dialog-backdrop">
      <section class="settings-dialog" role="dialog" aria-modal="true" aria-label="Receive transfer">
        <div class="dialog-title">
          <div>
            <p class="eyebrow">Incoming Transfer</p>
            <h2>{{ pendingOffer.clipboard_text ? "Incoming Clipboard Text" : "Receive Files?" }}</h2>
          </div>
        </div>

        <template v-if="pendingOffer.clipboard_text">
          <p class="incoming-summary">
            {{ pendingOffer.sender_alias }} wants to send clipboard text · {{ formatBytes(pendingOffer.clipboard_text.size) }}.
          </p>
          <pre class="clipboard-preview">{{ pendingOffer.clipboard_text.text }}</pre>
          <div class="dialog-actions">
            <button class="ghost-button" type="button" @click="decideIncomingOffer(false)">Decline</button>
            <button class="primary-button" type="button" @click="copyIncomingClipboardText">Copy</button>
          </div>
        </template>

        <template v-else>
          <p class="incoming-summary">
            {{ pendingOffer.sender_alias }} wants to send {{ pendingOffer.files.length }}
            file{{ pendingOffer.files.length === 1 ? "" : "s" }} · {{ formatBytes(pendingOffer.total_size) }}.
          </p>

          <div class="incoming-files">
            <div v-for="file in pendingOffer.files.slice(0, 4)" :key="file.index" class="file-pill">
              <span>{{ file.name }}</span>
              <strong>{{ formatBytes(file.size) }}</strong>
            </div>
          </div>

          <div class="dialog-actions">
            <button class="ghost-button" type="button" @click="decideIncomingOffer(false)">Decline</button>
            <button class="primary-button" type="button" @click="decideIncomingOffer(true)">Accept</button>
          </div>
        </template>
      </section>
    </div>

    <div v-if="isAddDeviceOpen" class="dialog-backdrop" @click.self="closePeerDialog">
      <section class="settings-dialog" role="dialog" aria-modal="true" :aria-label="editingPeerId ? 'Edit device' : 'Add device'">
        <div class="dialog-title">
          <div>
            <p class="eyebrow title-only">{{ editingPeerId ? "Edit Device" : "Add Device" }}</p>
          </div>
          <button class="close-button" type="button" @click="closePeerDialog">×</button>
        </div>

        <div class="type-toggle" aria-label="Address type">
          <button
            type="button"
            :class="{ active: newPeer.addressType === 'ipv4' }"
            @click="newPeer.addressType = 'ipv4'"
          >
            IPv4
          </button>
          <button
            type="button"
            :class="{ active: newPeer.addressType === 'ipv6' }"
            @click="newPeer.addressType = 'ipv6'"
          >
            IPv6
          </button>
        </div>

        <label>
          <span>Device Address</span>
          <input v-model="newPeer.host" :placeholder="newPeer.addressType === 'ipv4' ? '192.168.1.88' : 'fd00::1234'" />
        </label>

        <label>
          <span>Port</span>
          <input v-model.number="newPeer.port" type="number" min="1024" max="49151" />
        </label>

        <p v-if="peerMessage" class="dialog-message">{{ peerMessage }}</p>

        <div class="dialog-actions">
          <button class="ghost-button" type="button" @click="testNewPeer">Test</button>
          <button class="primary-button" type="button" @click="savePeerDialog">Save</button>
        </div>
      </section>
    </div>

    <div v-if="isSettingsOpen" class="dialog-backdrop" @click.self="isSettingsOpen = false">
      <section class="settings-dialog" role="dialog" aria-modal="true" aria-label="Settings">
        <div class="dialog-title">
          <div>
            <p class="eyebrow title-only">Settings</p>
          </div>
          <button class="close-button" type="button" @click="isSettingsOpen = false">×</button>
        </div>

        <label>
          <span>Alias</span>
          <input v-model="settings.alias" maxlength="32" placeholder="Letters, numbers, and Chinese characters only" />
        </label>

        <label>
          <span>Default Port</span>
          <input v-model.number="settings.port" type="number" min="1024" max="49151" />
        </label>

        <label>
          <span>Save Folder</span>
          <div class="path-row">
            <input v-model="settings.savePath" />
            <button type="button" @click="chooseSaveFolder">Browse</button>
          </div>
        </label>

        <label>
          <span>Auto Refresh Interval (seconds)</span>
          <input v-model.number="settings.refreshIntervalSeconds" type="number" min="5" max="3600" />
        </label>

        <div class="dialog-actions">
          <button class="ghost-button" type="button" @click="isSettingsOpen = false">Cancel</button>
          <button class="primary-button" type="button" @click="saveSettings">Save</button>
        </div>
      </section>
    </div>
  </main>
</template>

<style>
:root {
  color: #23302f;
  background: #e8e4d8;
  font-family:
    "LXGW WenKai Screen", "Noto Serif CJK SC", "Songti SC", Georgia, serif;
  font-size: 16px;
  font-synthesis: none;
  text-rendering: geometricPrecision;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
}

* {
  box-sizing: border-box;
}

html,
body,
#app {
  width: 100%;
  height: 100%;
  margin: 0;
}

button,
input {
  font: inherit;
}

button {
  cursor: pointer;
}

.app-shell {
  position: relative;
  display: flex;
  min-height: 760px;
  height: 100vh;
  overflow: hidden;
  padding: 28px;
  background:
    radial-gradient(circle at 20% 18%, rgba(109, 144, 122, 0.26), transparent 32%),
    radial-gradient(circle at 86% 8%, rgba(211, 146, 92, 0.28), transparent 28%),
    linear-gradient(135deg, #eee9dc 0%, #d9dfd3 48%, #efe2cf 100%);
}

.app-shell::before {
  position: absolute;
  inset: 0;
  pointer-events: none;
  content: "";
  opacity: 0.36;
  background-image:
    linear-gradient(rgba(35, 48, 47, 0.055) 1px, transparent 1px),
    linear-gradient(90deg, rgba(35, 48, 47, 0.055) 1px, transparent 1px);
  background-size: 28px 28px;
  mask-image: radial-gradient(circle at center, black, transparent 72%);
}

.ambient {
  position: absolute;
  border-radius: 999px;
  filter: blur(10px);
  opacity: 0.7;
}

.ambient-one {
  width: 260px;
  height: 260px;
  left: -90px;
  bottom: -70px;
  background: rgba(62, 113, 105, 0.22);
}

.ambient-two {
  width: 220px;
  height: 220px;
  right: -60px;
  top: 82px;
  background: rgba(185, 103, 64, 0.2);
}

.window-panel {
  position: relative;
  z-index: 1;
  display: grid;
  flex: 1;
  min-height: 704px;
  grid-template-rows: auto minmax(330px, 1fr) auto;
  gap: 22px;
  padding: 22px;
  border: 1px solid rgba(35, 48, 47, 0.14);
  border-radius: 34px;
  background: rgba(252, 249, 239, 0.74);
  box-shadow: 0 30px 90px rgba(65, 57, 42, 0.18), inset 0 1px 0 rgba(255, 255, 255, 0.78);
  backdrop-filter: blur(18px);
}

.top-bar,
.device-dock {
  border: 1px solid rgba(35, 48, 47, 0.1);
  background: rgba(255, 253, 246, 0.66);
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.8);
}

.top-bar {
  display: grid;
  grid-template-columns: auto 1fr auto;
  gap: 18px;
  align-items: center;
  padding: 14px;
  border-radius: 24px;
}

.brand-block,
.status-strip,
.dock-heading,
.device-grid,
.dialog-title,
.dialog-actions,
.path-row {
  display: flex;
  align-items: center;
}

.brand-block {
  gap: 12px;
}

.brand-mark {
  display: grid;
  width: 52px;
  height: 52px;
  place-items: center;
  border-radius: 18px;
  color: #f9f3df;
  background: #253b38;
  font-weight: 800;
  letter-spacing: 0.08em;
  box-shadow: 0 14px 28px rgba(37, 59, 56, 0.24);
}

.eyebrow {
  margin: 0 0 4px;
  color: #846348;
  font-size: 0.7rem;
  font-weight: 800;
  letter-spacing: 0.16em;
  text-transform: uppercase;
}

.eyebrow.title-only {
  margin-bottom: 0;
  color: #23302f;
  font-size: 0.95rem;
}

h1,
h2,
p {
  margin: 0;
}

h1 {
  font-size: 1.12rem;
  letter-spacing: 0.04em;
}

.status-strip {
  justify-content: flex-end;
  gap: 10px;
  min-width: 0;
}

.status-chip {
  min-width: 132px;
  padding: 9px 12px;
  border: 1px solid rgba(35, 48, 47, 0.1);
  border-radius: 16px;
  background: rgba(247, 241, 226, 0.66);
}

.status-chip.wide {
  min-width: 178px;
}

.status-chip span,
.status-chip strong {
  display: block;
}

.status-chip span {
  color: #7a827d;
  font-size: 0.72rem;
}

.status-chip strong {
  overflow: hidden;
  margin-top: 1px;
  color: #23302f;
  font-size: 0.88rem;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.alias-chip strong {
  color: #9a4d2e;
}

.icon-button,
.refresh-button,
.ghost-button,
.primary-button,
.path-row button,
.add-device-button,
.close-button,
.drop-clear-button {
  border: 0;
}

.icon-button,
.refresh-button,
.ghost-button,
.path-row button,
.drop-clear-button {
  color: #253b38;
  background: rgba(255, 255, 255, 0.68);
  border: 1px solid rgba(35, 48, 47, 0.12);
}

.icon-button {
  height: 44px;
  padding: 0 18px;
  border-radius: 15px;
  font-weight: 800;
}

.error-banner {
  margin: -10px 2px 0;
  padding: 10px 14px;
  border: 1px solid rgba(154, 77, 46, 0.28);
  border-radius: 16px;
  color: #8a3f27;
  background: rgba(255, 238, 222, 0.76);
  font-weight: 800;
}

.toast-bubble {
  position: fixed;
  z-index: 6;
  top: 26px;
  left: 50%;
  max-width: min(560px, calc(100vw - 48px));
  transform: translateX(-50%);
  padding: 12px 18px;
  border: 1px solid rgba(35, 48, 47, 0.12);
  border-radius: 999px;
  color: #f8eed8;
  background: rgba(37, 59, 56, 0.92);
  box-shadow: 0 18px 42px rgba(37, 59, 56, 0.24);
  font-weight: 900;
  text-align: center;
}

.toast-enter-active,
.toast-leave-active {
  transition: opacity 180ms ease, transform 180ms ease;
}

.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translate(-50%, -10px);
}

.drop-zone {
  position: relative;
  display: grid;
  align-items: start;
  justify-items: start;
  min-height: 330px;
  height: auto;
  overflow: hidden;
  padding: 28px;
  border: 1.5px dashed rgba(37, 59, 56, 0.22);
  border-radius: 34px;
  background:
    linear-gradient(135deg, rgba(255, 253, 246, 0.74), rgba(235, 236, 224, 0.58)),
    radial-gradient(circle at 72% 46%, rgba(37, 59, 56, 0.05), transparent 34%);
}

.drop-zone.has-files {
  border-style: solid;
  border-color: rgba(154, 77, 46, 0.34);
}

.drop-orbit {
  position: absolute;
  right: 9%;
  bottom: 8%;
  display: grid;
  width: 220px;
  height: 220px;
  place-items: center;
  opacity: 0.46;
}

.orbit-ring {
  position: absolute;
  border: 1px solid rgba(37, 59, 56, 0.13);
  border-radius: 48% 52% 55% 45%;
}

.ring-one {
  width: 230px;
  height: 164px;
  transform: rotate(-12deg);
}

.ring-two {
  width: 164px;
  height: 230px;
  transform: rotate(18deg);
}

.drop-core {
  display: grid;
  width: 70px;
  height: 70px;
  place-items: center;
  border-radius: 26px;
  color: #253b38;
  background: rgba(255, 253, 246, 0.68);
  box-shadow: 0 18px 42px rgba(37, 59, 56, 0.12);
  font-size: 1.7rem;
}

.drop-copy {
  position: relative;
  z-index: 1;
  max-width: 360px;
  text-align: left;
}

.drop-copy h2 {
  color: #1e2f2c;
  font-size: clamp(1.7rem, 3.8vw, 3rem);
  line-height: 1;
  letter-spacing: -0.06em;
}

.drop-summary {
  max-width: 360px;
  margin: 10px 0 0;
  color: #65706a;
  font-size: 0.88rem;
}

.drop-actions {
  position: absolute;
  z-index: 2;
  top: 28px;
  right: 28px;
  display: flex;
  gap: 8px;
}

.drop-clear-button {
  position: relative;
}

.file-stack {
  position: relative;
  z-index: 1;
  display: flex;
  max-height: 210px;
  flex-wrap: wrap;
  align-items: flex-start;
  gap: 8px;
  width: min(100%, 760px);
  margin-top: 18px;
  overflow-y: auto;
  overscroll-behavior: contain;
  padding: 0 8px 8px 0;
}

.file-pill {
  position: relative;
  display: flex;
  max-width: 240px;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 12px;
  border-radius: 14px;
  background: rgba(255, 253, 246, 0.86);
  box-shadow: 0 10px 28px rgba(65, 57, 42, 0.11);
}

.selected-file-pill {
  padding-right: 30px;
}

.clipboard-pill {
  border: 1px solid rgba(154, 77, 46, 0.22);
  background: rgba(255, 244, 223, 0.9);
}

.file-remove-button {
  position: absolute;
  top: 3px;
  right: 7px;
  display: grid;
  width: 18px;
  height: 18px;
  place-items: center;
  border: 0;
  color: #c83824;
  background: transparent;
  font-size: 1rem;
  font-weight: 950;
  line-height: 1;
}

.file-pill span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-pill strong {
  flex: none;
  color: #9a4d2e;
}

.device-dock {
  padding: 16px;
  border-radius: 26px;
}

.dock-heading {
  justify-content: space-between;
  margin-bottom: 14px;
}

.dock-heading h2 {
  font-size: 1.1rem;
}

.refresh-button,
.ghost-button,
.primary-button,
.path-row button,
.drop-clear-button {
  height: 38px;
  padding: 0 14px;
  border-radius: 999px;
  font-weight: 800;
}

.device-grid {
  gap: 14px;
  overflow-x: auto;
  overflow-y: hidden;
  overscroll-behavior-x: contain;
  padding-bottom: 8px;
  white-space: nowrap;
}

.empty-devices {
  padding: 22px 10px;
  color: #78817c;
  font-weight: 800;
}

.device-card {
  display: grid;
  flex: 0 0 auto;
  min-width: 118px;
  justify-items: center;
  gap: 7px;
  padding: 12px;
  border: 1px solid rgba(35, 48, 47, 0.1);
  border-radius: 22px;
  color: #23302f;
  background: rgba(255, 253, 246, 0.52);
  transition: transform 160ms ease, background 160ms ease;
}

.device-card:hover {
  background: rgba(255, 253, 246, 0.88);
  transform: translateY(-2px);
}

.device-card:disabled {
  cursor: wait;
  opacity: 0.62;
}

.avatar {
  position: relative;
  display: grid;
  width: 62px;
  height: 62px;
  place-items: center;
  border-radius: 999px;
  color: #f8eed8;
  background: linear-gradient(145deg, #b96740, #253b38);
  box-shadow: 0 14px 26px rgba(65, 57, 42, 0.16);
  font-size: 1.35rem;
  font-weight: 900;
}

.status-dot {
  position: absolute;
  right: 2px;
  bottom: 5px;
  width: 14px;
  height: 14px;
  border: 3px solid #fff9ea;
  border-radius: 999px;
}

.status-dot.online {
  background: #48a46b;
}

.status-dot.unknown {
  background: #d8a23d;
}

.status-dot.offline {
  background: #9ca29e;
}

.device-card small {
  max-width: 94px;
  overflow: hidden;
  color: #78817c;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.add-device-button {
  position: fixed;
  z-index: 3;
  right: 34px;
  bottom: 34px;
  display: flex;
  align-items: center;
  gap: 9px;
  height: 54px;
  padding: 0 20px 0 14px;
  border-radius: 999px;
  color: #f8eed8;
  background: #253b38;
  box-shadow: 0 18px 42px rgba(37, 59, 56, 0.28);
  font-weight: 900;
}

.add-device-button span {
  display: grid;
  width: 30px;
  height: 30px;
  place-items: center;
  border-radius: 999px;
  color: #253b38;
  background: #f8eed8;
  font-size: 1.2rem;
}

.dialog-backdrop {
  position: fixed;
  z-index: 4;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 24px;
  background: rgba(28, 35, 33, 0.34);
  backdrop-filter: blur(10px);
}

.settings-dialog {
  width: min(520px, 100%);
  padding: 22px;
  border: 1px solid rgba(35, 48, 47, 0.14);
  border-radius: 28px;
  background: #fff9ea;
  box-shadow: 0 36px 90px rgba(23, 33, 31, 0.28);
}

.dialog-title {
  justify-content: space-between;
  margin-bottom: 22px;
}

.close-button {
  width: 38px;
  height: 38px;
  border-radius: 999px;
  color: #253b38;
  background: rgba(37, 59, 56, 0.08);
  font-size: 1.45rem;
}

.type-toggle {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
  padding: 5px;
  border-radius: 18px;
  background: rgba(37, 59, 56, 0.08);
}

.type-toggle button {
  height: 40px;
  border: 0;
  border-radius: 14px;
  color: #57615c;
  background: transparent;
  font-weight: 900;
}

.type-toggle button.active {
  color: #f8eed8;
  background: #253b38;
  box-shadow: 0 10px 22px rgba(37, 59, 56, 0.18);
}

.dialog-message,
.incoming-summary {
  margin-top: 16px;
  padding: 10px 12px;
  border-radius: 14px;
  color: #57615c;
  background: rgba(37, 59, 56, 0.08);
  font-weight: 800;
}

.incoming-files {
  display: grid;
  gap: 8px;
  margin-top: 12px;
}

.send-files-preview {
  max-height: 190px;
  overflow-y: auto;
  padding-right: 6px;
}

.clipboard-preview {
  max-height: 240px;
  overflow: auto;
  margin: 12px 0 0;
  padding: 14px;
  border: 1px solid rgba(35, 48, 47, 0.1);
  border-radius: 16px;
  color: #23302f;
  background: rgba(255, 255, 255, 0.72);
  font: 0.9rem/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  white-space: pre-wrap;
  word-break: break-word;
}

.settings-dialog label {
  display: grid;
  gap: 8px;
  margin-top: 16px;
  color: #57615c;
  font-size: 0.9rem;
  font-weight: 800;
}

.settings-dialog input {
  width: 100%;
  height: 46px;
  border: 1px solid rgba(35, 48, 47, 0.12);
  border-radius: 15px;
  outline: none;
  padding: 0 14px;
  color: #23302f;
  background: rgba(255, 255, 255, 0.72);
}

.settings-dialog input:focus {
  border-color: rgba(154, 77, 46, 0.55);
  box-shadow: 0 0 0 4px rgba(154, 77, 46, 0.11);
}

.path-row {
  gap: 8px;
}

.path-row button {
  flex: none;
}

.dialog-actions {
  justify-content: flex-end;
  gap: 10px;
  margin-top: 24px;
}

.primary-button {
  color: #f8eed8;
  background: #253b38;
}

.danger-button {
  background: #9a4d2e;
}

@media (max-width: 940px) {
  .top-bar {
    grid-template-columns: 1fr auto;
  }

  .status-strip {
    grid-column: 1 / -1;
    justify-content: flex-start;
    overflow-x: auto;
  }
}

@media (max-width: 700px) {
  .app-shell {
    padding: 14px;
  }

  .window-panel {
    min-height: calc(100vh - 28px);
    padding: 14px;
    border-radius: 24px;
  }

  .top-bar {
    gap: 12px;
  }

  .brand-mark {
    width: 44px;
    height: 44px;
    border-radius: 15px;
  }

  .drop-zone {
    min-height: 300px;
  }

  .drop-copy h2 {
    font-size: 2.6rem;
  }

  .add-device-button {
    right: 20px;
    bottom: 20px;
  }
}
</style>
