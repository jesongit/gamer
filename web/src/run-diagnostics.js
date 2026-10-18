// Native same-origin downloads stream to disk without buffering a ZIP in JS.
export function runDiagnosticsUrl(runId) {
  return runId ? `/api/runs/${encodeURIComponent(runId)}/diagnostics` : ''
}
export function runDiagnosticsFilename(runId) {
  return `gamer-diagnostics-${String(runId || '').replace(/[^a-zA-Z0-9._-]/g, '_').slice(0, 128)}.zip`
}
