// The changelog (Caveman/CHANGELOG.md, bundled at build time) read into entries: the front page shows the newest one's
// bullets; the Changelogs dialog shows them all.

import raw from '../../../CHANGELOG.md?raw';

export interface Entry {
  version: string;
  date: string;
  bullets: string[];
}

export function parse(text: string): Entry[] {
  const out: Entry[] = [];
  let current: Entry | null = null;
  for (const line of text.split(/\r?\n/)) {
    const head = /^##\s+(\S+)(?:\s+\(([^)]*)\))?/.exec(line);
    if (head) {
      current = { version: head[1], date: head[2] ?? '', bullets: [] };
      out.push(current);
      continue;
    }
    const bullet = /^-\s+(.*)$/.exec(line);
    if (bullet && current) current.bullets.push(bullet[1].trim());
  }
  return out;
}

export const ENTRIES: Entry[] = parse(raw);
export const LATEST: Entry | null = ENTRIES[0] ?? null;
export const TEXT = raw;
