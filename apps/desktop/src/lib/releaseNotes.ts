// Release notes as the update dialog shows them. The notes are plain text from the release
// (docs/release-notes/<version>.md, through latest.json): a run of lines starting with "- " is a
// list, any other non-empty line a paragraph. They are rendered as text, never as HTML.

export type ReleaseNotesBlock = { kind: "list"; items: string[] } | { kind: "paragraph"; text: string };

export function parseReleaseNotes(text: string | null): ReleaseNotesBlock[] {
  const blocks: ReleaseNotesBlock[] = [];
  for (const raw of (text ?? "").split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) continue;
    if (line.startsWith("- ")) {
      const item = line.slice(2).trim();
      if (!item) continue;
      const last = blocks[blocks.length - 1];
      if (last?.kind === "list") last.items.push(item);
      else blocks.push({ kind: "list", items: [item] });
    } else {
      blocks.push({ kind: "paragraph", text: line });
    }
  }
  return blocks;
}
