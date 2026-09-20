import { reactive } from 'vue'

// Shared, in-memory navigation requests. Plugin modules subscribe by address;
// messages do not carry Vue components, device handles, or resource contents.
const channels = new Map()
export function pluginMessageChannel(address) {
  if (!channels.has(address)) channels.set(address, reactive({ seq: 0, packageId: '', scriptId: '' }))
  return channels.get(address)
}
