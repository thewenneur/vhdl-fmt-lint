// Prepares a page of the HTML documentation `vfl doc` writes for the
// diagrams webview. Kept free of `vscode` so that it can be unit tested.

/** The command the links between pages run in the webview. */
export const OPEN_PAGE_COMMAND = "vfl.openDocPage";

/**
 * Nothing loaded: the pages are self-contained HTML with inline SVG. The
 * only script allowed is the diagram zoom `vfl doc` puts last in the body,
 * marked with `nonce`.
 */
function csp(nonce: string): string {
  return `default-src 'none'; style-src 'unsafe-inline'; img-src data:; script-src 'nonce-${nonce}';`;
}

/**
 * `html` for the webview: a restrictive content security policy, the dark
 * styles applied when `dark` (and only then, whatever the OS theme), and
 * links to the other pages turned into command links to `page`'s siblings,
 * and the zoom script of the diagrams allowed to run.
 */
export function webviewHtml(html: string, page: string, dark: boolean, nonce: string): string {
  // The pages follow `prefers-color-scheme`, which webviews take from the
  // OS rather than from the VS Code theme.
  let result = html.replace(/@media\s*\(\s*prefers-color-scheme\s*:\s*dark\s*\)/g, () =>
    dark ? "@media all" : "@media not all",
  );
  result = result.replace(
    /:root\s*\{\s*color-scheme\s*:\s*light dark\s*;?/,
    (m) => m.replace("light dark", dark ? "dark" : "light"),
  );
  // Relative links to other pages, such as `work.fifo.html#arch-rtl`.
  result = result.replace(
    /(\s(?:xlink:)?href=")([^"#:/\\]+\.html)(#[^"]*)?"/g,
    (_m, attr: string, target: string, anchor?: string) => {
      if (target === page) {
        return `${attr}${anchor ?? "#"}"`;
      }
      const args = encodeURIComponent(JSON.stringify([target]));
      return `${attr}command:${OPEN_PAGE_COMMAND}?${args}"`;
    },
  );
  // Only the script vfl writes right before `</body>`; text from the
  // sources is escaped, so it cannot add one.
  result = result.replace(
    /<script>((?:(?!<\/script>)[\s\S])*)<\/script>(\s*<\/body>)/,
    (_m, code: string, end: string) => `<script nonce="${nonce}">${code}</script>${end}`,
  );
  const meta = `<meta http-equiv="Content-Security-Policy" content="${csp(nonce)}">`;
  const head = /<head[^>]*>/i.exec(result);
  if (head) {
    const at = head.index + head[0].length;
    return result.slice(0, at) + "\n" + meta + result.slice(at);
  }
  return meta + "\n" + result;
}

/** The title of the page `library.name.html`: `name`. */
export function pageTitle(page: string): string {
  const stem = page.replace(/\.html$/, "");
  if (stem === "index") {
    return "Documentation";
  }
  const dot = stem.indexOf(".");
  return dot >= 0 ? stem.slice(dot + 1) : stem;
}
