<template>
  <DeviceStage v-bind="$attrs" :connected="connected" :connecting="connecting" :error-msg="errorMsg"
    :browser-preview="isBrowser ? browser.view : null" :on-browser-loaded="browser.loaded" :on-browser-error="browser.failed"
    :audio-muted="forceMuted || !selected || audioMuted" :flush-and-connect="connect" :fullscreen="fullscreen"
    @video-mounted="attachVideo" @wrap-mounted="attachWrap" @loupe-mounted="attachLoupe" />
</template>
<script setup>
// Keep the user's preview choice separate from the transport. Hidden targets
// release their viewer and decoder, while accounts and server runs keep living.
import { computed, onUnmounted, ref, shallowRef, watch } from 'vue'
import DeviceStage from '../../workspace/DeviceStage.vue'
import { api } from '../../api'
import { useWebRtcLifecycle } from '../../composables/useWebRtcLifecycle'
import { useBrowserPreview } from './useBrowserPreview'
import { appStartedDevices, useToast } from '../../store'
defineOptions({ inheritAttrs: false })
const props = defineProps({ targetId: { type: String, required: true }, selected: Boolean, forceMuted: Boolean,
  active: { type: Boolean, default: true }, previewMode: { type: String, default: 'single' } })
const emit = defineEmits(['register', 'control-message', 'connected', 'video-mounted', 'wrap-mounted', 'loupe-mounted'])
// The component is keyed by targetId; capture an immutable identity for all late callbacks.
const id = props.targetId
const isBrowser = id.startsWith('browser-')
const connected = ref(false), connecting = ref(false), errorMsg = ref('')
const resolution = ref('—')
const superseded = ref(false), manualClose = ref(false), audioMuted = ref(true)
const previewWanted = ref(false)
const suspended = computed(() => previewWanted.value && !props.active)
const videoElement = shallowRef(null), videoWrap = shallowRef(null), loupeElement = shallowRef(null)
const toast = useToast()
let stream = null
function reportEvent(event) { emit('control-message', id, event) }
const browser = useBrowserPreview({ deviceId: () => id, connected, connecting, errorMsg, toast,
  previewMode: () => props.previewMode,
  onEvent: data => reportEvent({ data: JSON.stringify(data) }),
  onConnected: () => emit('connected', id),
  onDisconnected: clearVideo,
})
function setMuted(muted = true) {
  audioMuted.value = muted || !props.selected || props.forceMuted
  if (videoElement.value) videoElement.value.muted = audioMuted.value
  for (const track of stream?.getAudioTracks?.() || []) track.enabled = !audioMuted.value
  const channel = rtc.getControlChannel()
  if (channel?.readyState === 'open') channel.send(JSON.stringify({ type: 'audio', on: !audioMuted.value }))
}
const rtc = useWebRtcLifecycle({ api: { ...api, connectDevice: async target => {
  const result = await api.connectDevice(target)
  if (result?.app_started) appStartedDevices.add(target)
  return result
} }, deviceIdRef: computed(() => id), connectedRef: connected, connectingRef: connecting,
  errorMsgRef: errorMsg, supersededRef: superseded, manualCloseRef: manualClose, toast,
  onChannelOpen() { connected.value = true; connecting.value = false; setMuted(audioMuted.value) },
  onChannelClose() { connected.value = false },
  onConnectSuccess() { emit('connected', id) },
  onDisconnect() { connected.value = false; clearVideo() },
  onPeerDisposed() { for (const track of stream?.getTracks?.() || []) track.stop?.(); stream = null },
  onControlMessage: reportEvent,
  onRemoteTrack({ event, pc }) {
    if (event.target !== pc) return
    stream = event.streams[0] || stream || new MediaStream()
    if (!stream.getTracks().includes(event.track)) stream.addTrack(event.track)
    if (event.track.kind === 'audio') event.track.enabled = props.selected && !audioMuted.value && !props.forceMuted
    attachStream()
  },
})
function clearVideo() {
  resolution.value = '—'
  if (!videoElement.value) return
  if (isBrowser) {
    // Vue removes the img on the next render. Drop our DOM reference and its
    // decoded pixels immediately, including while the tab itself is hidden.
    videoElement.value.removeEventListener?.('loadedmetadata', updateResolution)
    videoElement.value.removeAttribute?.('src')
    videoElement.value = null
    emit('video-mounted', id, null)
    return
  }
  videoElement.value.pause?.()
  videoElement.value.srcObject = null
  videoElement.value.removeAttribute?.('src')
  videoElement.value.load?.()
}
function attachStream() {
  if (videoElement.value && !isBrowser && stream) {
    videoElement.value.srcObject = stream
    videoElement.value.muted = props.forceMuted || !props.selected || audioMuted.value
    videoElement.value.play()?.catch(() => {})
  }
}
function updateResolution() {
  const el = videoElement.value
  const width = el?.videoWidth || el?.naturalWidth, height = el?.videoHeight || el?.naturalHeight
  if (width && height) resolution.value = `${width}×${height}`
}
function attachVideo(el) {
  videoElement.value?.removeEventListener?.('loadedmetadata', updateResolution)
  videoElement.value = el
  el?.addEventListener?.('loadedmetadata', updateResolution)
  updateResolution(); attachStream(); emit('video-mounted', id, el)
}
function attachLoupe(el) { loupeElement.value = el; emit('loupe-mounted', el) }
function attachWrap(el) { videoWrap.value = el; emit('wrap-mounted', id, el) }
async function connect(manual = true) {
  previewWanted.value = true
  if (!props.active) return
  if (connected.value || connecting.value) return
  if (isBrowser) await browser.connect()
  else await rtc.connect(manual)
}
function stopTransport(manual = false) {
  setMuted(true)
  browser.release()
  browser.close()
  rtc.cleanup(manual)
  clearVideo()
  resolution.value = '—'
}
function close() {
  previewWanted.value = false
  stopTransport(true)
}
function fullscreen() { videoWrap.value?.requestFullscreen?.() }
watch(() => props.selected, selected => { if (!selected) setMuted(true) }, { flush: 'sync' })
watch(() => props.forceMuted, muted => { if (muted) setMuted(true) }, { flush: 'sync', immediate: true })
watch(() => props.active, active => {
  if (!active) stopTransport(false)
  else if (previewWanted.value && !superseded.value) connect(false)
}, { flush: 'sync' })
const session = { id, connected, connecting, errorMsg, superseded, manualClose, audioMuted, videoElement, videoWrap, loupeElement,
  browser, rtc, connect, close, setMuted, fullscreen, resolution, previewWanted, suspended }
emit('register', id, session)
onUnmounted(() => { videoElement.value?.removeEventListener?.('loadedmetadata', updateResolution); close(); emit('register', id, null) })
</script>
