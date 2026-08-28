/**
 * Render an alt-mode operation template with the supplied placeholder values.
 *
 * Empty placeholders on an otherwise blank line are removed so optional
 * values do not leave invalid YAML indentation behind. Newline-containing
 * values inherit the placeholder line's indentation.
 */
export function renderOpTpl(tpl, vars = {}) {
  let out = tpl || ''
  for (const [k, v] of Object.entries(vars)) {
    const val = v ?? ''
    out = out.split('\n').map(line => {
      if (!line.includes('{' + k + '}')) return line
      if (String(val).trim() === '' && line.replace('{' + k + '}', '').trim() === '') return null
      const indent = (line.match(/^(\s*)/) || ['', ''])[1]
      return line.split('{' + k + '}').join(val.replace(/\n\s*/g, '\n' + indent + '  '))
    }).filter(l => l !== null).join('\n')
  }
  return out.replace(/\n{3,}/g, '\n\n').trim()
}
