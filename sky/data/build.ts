// Writes `bright_stars.tsv`, the sky's stars, from the Yale Bright Star Catalogue (5th revised edition; Hoffleit and
// Warren 1991), CDS catalogue V/50: https://cdsarc.cds.unistra.fr/ftp/cats/V/50/catalog.gz
//
//   bun native/crates/naval-sky/data/build.ts [path to the unpacked `catalog`]
//
// Without a path it downloads the catalogue to `.build/sky/`. The table is the catalogue's J2000 places, V magnitudes
// and B−V colours, brightest first, with two changes: the fourteen entries the catalogue itself withdrew (novae and
// clusters, no place given) are left out, and components closer than `MERGE_ARCSEC` (Alpha Centauri, Castor, Mizar)
// are one star of their summed light at the brighter one's place, as the eye sees them. A star with no B−V takes its
// spectral class's.
import { gunzipSync } from 'node:zlib';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';

const SOURCE = 'https://cdsarc.cds.unistra.fr/ftp/cats/V/50/catalog.gz';
const MERGE_ARCSEC = 90;
// B−V of the main sequence by spectral class, for the few stars the catalogue gives no colour.
const CLASS_COLOR: Record<string, number> = { O: -0.3, B: -0.15, A: 0.1, F: 0.42, G: 0.68, K: 1.1, M: 1.55, N: 1.8, S: 1.8, C: 1.8, W: -0.3 };

const root = join(import.meta.dir, '../../../..');
const out = join(import.meta.dir, 'bright_stars.tsv');

async function catalogue(): Promise<string> {
  const given = process.argv[2];
  if (given) return readFile(given, 'latin1');
  const cached = join(root, '.build/sky/bsc5-catalog');
  try {
    return await readFile(cached, 'latin1');
  } catch {
    const response = await fetch(SOURCE);
    if (!response.ok) throw new Error(`${SOURCE}: ${response.status}`);
    const text = gunzipSync(Buffer.from(await response.arrayBuffer())).toString('latin1');
    await mkdir(dirname(cached), { recursive: true });
    await writeFile(cached, text, 'latin1');
    return text;
  }
}

type Star = { hr: number; ra: number; dec: number; magnitude: number; color: number };

// Columns are the catalogue's byte ranges, 1-based and inclusive (its ReadMe).
const field = (line: string, from: number, to: number) => line.slice(from - 1, to).trim();

function parse(line: string): Star | null {
  const hours = field(line, 76, 77);
  const magnitude = field(line, 103, 107);
  if (!hours || !magnitude) return null;
  const ra = (Number(hours) + Number(field(line, 78, 79)) / 60 + Number(field(line, 80, 83)) / 3600) * 15;
  const sign = field(line, 84, 84) === '-' ? -1 : 1;
  const dec = sign * (Number(field(line, 85, 86)) + Number(field(line, 87, 88)) / 60 + Number(field(line, 89, 90)) / 3600);
  const color = field(line, 110, 114);
  const spectral = field(line, 128, 147).replace(/^[a-z]+/, '');
  return {
    hr: Number(field(line, 1, 4)),
    ra,
    dec,
    magnitude: Number(magnitude),
    color: color ? Number(color) : (CLASS_COLOR[spectral[0]] ?? 0.6),
  };
}

const flux = (magnitude: number) => 10 ** (-0.4 * magnitude);
const radians = (degrees: number) => (degrees * Math.PI) / 180;

function separation(a: Star, b: Star): number {
  const [d1, d2, dra] = [radians(a.dec), radians(b.dec), radians(a.ra - b.ra)];
  const h = Math.sin((d1 - d2) / 2) ** 2 + Math.cos(d1) * Math.cos(d2) * Math.sin(dra / 2) ** 2;
  return ((2 * Math.asin(Math.sqrt(h)) * 180) / Math.PI) * 3600;
}

const stars = (await catalogue())
  .split('\n')
  .map(parse)
  .filter((star): star is Star => star !== null)
  .sort((a, b) => a.magnitude - b.magnitude || a.hr - b.hr);

// Brightest first, each fainter component within reach folded into the star already kept.
const kept: Star[] = [];
const byDec = new Map<number, Star[]>();
let merged = 0;
for (const star of stars) {
  const band = Math.floor(star.dec * 10);
  const near = [band - 1, band, band + 1].flatMap((b) => byDec.get(b) ?? []);
  const primary = near.find((other) => separation(star, other) < MERGE_ARCSEC);
  if (primary) {
    const [a, b] = [flux(primary.magnitude), flux(star.magnitude)];
    primary.color = (primary.color * a + star.color * b) / (a + b);
    primary.magnitude = -2.5 * Math.log10(a + b);
    merged++;
    continue;
  }
  kept.push(star);
  byDec.set(band, [...(byDec.get(band) ?? []), star]);
}
kept.sort((a, b) => a.magnitude - b.magnitude || a.hr - b.hr);

const header = [
  '# The Yale Bright Star Catalogue, 5th revised edition (Hoffleit and Warren 1991; CDS V/50), as the sky draws it.',
  '# Written by build.ts beside this file: do not edit by hand.',
  `# ${kept.length} stars, brightest first; ${merged} close components folded into their primaries.`,
  '# HR\tright ascension (degrees, J2000)\tdeclination (degrees, J2000)\tV magnitude\tB-V',
];
const rows = kept.map((s) => [s.hr, s.ra.toFixed(5), s.dec.toFixed(5), s.magnitude.toFixed(2), s.color.toFixed(2)].join('\t'));
await writeFile(out, [...header, ...rows].join('\n') + '\n');
console.log(`${out}: ${kept.length} stars, ${merged} merged`);
