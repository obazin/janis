#!/usr/bin/env node
// Rebuilds the web-radio catalog at src/lib/features/radio/stations.json.
//
// It merges two sources:
//   1. curated.json — the hand-picked flagship stations (SomaFM, Radio France,
//      Radio Paradise, …), kept verbatim including their now-playing wiring.
//   2. The Radio Browser community database (https://www.radio-browser.info),
//      the most-listened working stations per target country, plus tag-targeted
//      top-ups for a few genres (see GENRE_PULLS), fetched live, deduplicated,
//      and mapped onto our i18n genre keys.
//
// The result is static data baked into the app — nothing here runs at runtime.
// Run it to refresh the catalog:  node scripts/radio/build-stations.mjs
//
// New genre keys must also exist in src/lib/i18n/{en,fr}.json (the TranslationKey
// type is derived from en.json), and new country codes need a radio.country.<cc>
// key in both. See scripts/radio/README.md.

import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, '..', '..', 'src', 'lib', 'features', 'radio', 'stations.json');
const USER_AGENT = 'Janis/0.1 (+https://github.com/obazin/janis)';
const MIRRORS = ['https://de1.api.radio-browser.info', 'https://nl1.api.radio-browser.info'];

// How many freshly pulled stations to keep per country (the curated seed adds
// on top). Sums to ~880; with the ~100 curated that lands the catalog near 1000.
const CAPS = {
    US: 130, GB: 80, DE: 80, FR: 55, IT: 55, ES: 50, BR: 45, NL: 45, PL: 40, GR: 35,
    BE: 30, SE: 30, PT: 30, CH: 25, AT: 25, IE: 25, DK: 25, NO: 25, FI: 25,
    CZ: 25, RO: 25, HU: 20,
};

// Tag-targeted top-ups for genres the per-country lists leave thin (a country's
// most-clicked stations skew pop/news). Pulled by exact tag, filtered to the
// CAPS countries, and labelled with the pulled genre outright.
const GENRE_PULLS = [
    { tag: 'blues', genre: 'radio.genre.blues', cap: 45 },
    { tag: 'funk', genre: 'radio.genre.funk', cap: 35 },
    { tag: 'jazz', genre: 'radio.genre.jazz', cap: 45 },
    { tag: 'rock', genre: 'radio.genre.rock', cap: 45 },
    { tag: 'pop', genre: 'radio.genre.pop', cap: 45 },
    { tag: 'classical', genre: 'radio.genre.classical', cap: 45 },
    { tag: 'bossa nova', genre: 'radio.genre.bossanova', cap: 30 },
];

// Genre scoring: map crowd tags -> our TranslationKey genre set. Order is the
// tie-break priority (earlier wins ties); score = # of a station's tags that
// match any keyword for that genre.
const GENRE_RULES = [
    ['radio.genre.dnb', ['drum and bass', 'drum & bass', 'dnb', 'jungle', 'liquid']],
    ['radio.genre.dubstep', ['dubstep']],
    ['radio.genre.bass', ['bass music', 'bassline', 'breakbeat', 'breaks']],
    ['radio.genre.trance', ['trance', 'psytrance', 'goa']],
    ['radio.genre.techno', ['techno', 'tech house', 'minimal']],
    ['radio.genre.house', ['house', 'deep house', 'progressive house']],
    ['radio.genre.synthwave', ['synthwave', 'synth-wave', 'synthpop', 'retrowave', 'darkwave']],
    ['radio.genre.vaporwave', ['vaporwave']],
    ['radio.genre.idm', ['idm']],
    ['radio.genre.industrial', ['industrial', 'ebm', 'gothic']],
    ['radio.genre.jazz', ['jazz', 'smooth jazz', 'bebop', 'swing', 'bigband', 'big band']],
    ['radio.genre.blues', ['blues']],
    ['radio.genre.soul', ['soul', 'rnb', 'r&b', 'r n b', 'motown', 'neo soul']],
    ['radio.genre.funk', ['funk', 'disco', 'groove', 'boogie']],
    ['radio.genre.hiphop', ['hip hop', 'hip-hop', 'hiphop', 'rap', 'trap', 'urban']],
    ['radio.genre.reggae', ['reggae', 'dancehall', 'ska', 'dub', 'roots']],
    ['radio.genre.metal', ['metal', 'heavy metal']],
    ['radio.genre.rock', ['rock', 'classic rock', 'hard rock', 'punk', 'grunge', 'indie rock', 'alternative rock', 'rockabilly']],
    ['radio.genre.indie', ['indie', 'alternative', 'shoegaze', 'new wave']],
    ['radio.genre.country', ['country', 'bluegrass', 'honky']],
    ['radio.genre.americana', ['americana', 'roots rock']],
    ['radio.genre.folk', ['folk', 'singer-songwriter', 'singer/songwriter', 'acoustic']],
    ['radio.genre.celtic', ['celtic', 'irish']],
    ['radio.genre.bossanova', ['bossa nova', 'bossa']],
    ['radio.genre.latin', ['latin', 'salsa', 'reggaeton', 'cumbia', 'flamenco']],
    ['radio.genre.classical', ['classical', 'opera', 'baroque', 'symphony', 'orchestral', 'klassik']],
    ['radio.genre.soundtrack', ['soundtrack', 'film score', 'game music', 'anime']],
    ['radio.genre.world', ['world', 'world music', 'ethnic', 'afrobeat']],
    ['radio.genre.electronic', ['electronic', 'electronica', 'downtempo', 'trip hop', 'trip-hop', 'ambient techno']],
    ['radio.genre.dance', ['dance', 'edm', 'electro', 'club', 'electronic dance', 'eurodance', 'pop dance']],
    ['radio.genre.ambient', ['ambient', 'drone', 'meditation']],
    ['radio.genre.chillout', ['chillout', 'chill out', 'chill', 'relax', 'lofi', 'lo-fi']],
    ['radio.genre.lounge', ['lounge', 'easy listening', 'easy-listening']],
    ['radio.genre.christian', ['christian', 'gospel', 'worship', 'religious', 'catholic', 'christ']],
    ['radio.genre.pop', ['pop', 'adult contemporary', 'contemporary', 'schlager', 'kpop', 'k-pop']],
    ['radio.genre.hits', ['hits', 'top 40', 'top40', 'top hits', 'charts', 'chart', 'top 100', 'greatest hits']],
    ['radio.genre.eighties', ['80s', "80's", '80er', '1980s', 'eighties']],
    ['radio.genre.nineties', ['90s', "90's", '90er', '1990s', 'nineties', '2000s', '00s', '2000er', '00er']],
    ['radio.genre.oldies', ['oldies', '60s', '70s', "60's", "70's", '1960s', '1970s', '60er', '70er', 'retro', 'evergreen', 'nostalgie']],
    ['radio.genre.news', ['news', 'information', 'local news', 'news talk', 'actualités', 'aktuell']],
    ['radio.genre.talk', ['talk', 'culture', 'spoken', 'sport', 'sports', 'politics', 'comedy', 'podcast']],
    ['radio.genre.variety', ['variety', 'music', 'public radio', 'regional', 'community', 'mixed', 'various']],
];

function classifyGenre(tagsStr, name) {
    const tags = (tagsStr || '').split(',').map((t) => t.trim().toLowerCase()).filter(Boolean);
    let best = null, bestScore = 0;
    for (const [genre, kws] of GENRE_RULES) {
        let score = 0;
        for (const tag of tags) if (kws.some((k) => tag.includes(k))) score++;
        if (score > bestScore) { bestScore = score; best = genre; }
    }
    if (best) return best;
    const n = (name || '').toLowerCase();
    for (const [genre, kws] of GENRE_RULES) if (kws.some((k) => n.includes(k))) return genre;
    return 'radio.genre.variety';
}

// Some stations report bitrate in bps, or 0; normalize to sane kbps.
function sanitizeKbps(k) {
    if (!k || k <= 0) return 128;
    if (k >= 10000) k = Math.round(k / 1000);
    return Math.min(k, 640);
}

// Stable 0..5 gradient index from the id.
function grad(id) {
    let h = 0;
    for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
    return h % 6;
}

// Normalized name for dedup within a country (collapse bitrate/codec variants).
function normName(name) {
    return name.toLowerCase()
        .replace(/\b(mp3|aac\+?|aacp|ogg|flac|opus|hls|128k?|192k?|256k?|320k?|64k?|96k?|48k?|\d{2,3}\s?kbps)\b/g, '')
        .replace(/[|(){}\[\]\-–—.]/g, ' ')
        .replace(/[^a-z0-9äöüßàâçéèêëîïôûùüÿñæœ ]/g, '')
        .replace(/\s+/g, ' ').trim();
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function fetchApi(path, label) {
    let lastErr;
    for (const base of MIRRORS) {
        try {
            const res = await fetch(base + path, { headers: { 'User-Agent': USER_AGENT } });
            if (!res.ok) throw new Error(`HTTP ${res.status}`);
            return await res.json();
        } catch (e) {
            lastErr = e;
        }
    }
    throw new Error(`fetch ${label} failed: ${lastErr}`);
}

const fetchCountry = (cc) =>
    fetchApi(
        `/json/stations/bycountrycodeexact/${cc}?order=clickcount&reverse=true&hidebroken=true&limit=150`,
        cc,
    );

// A deeper pool for tag pulls — most global results fall outside our countries,
// so cast wide and let the CAPS filter in `absorb` narrow it.
const fetchTag = (tag) =>
    fetchApi(
        `/json/stations/bytagexact/${encodeURIComponent(tag)}?order=clickcount&reverse=true&hidebroken=true&limit=300`,
        tag,
    );

async function main() {
    const curated = JSON.parse(readFileSync(join(HERE, 'curated.json'), 'utf8'));
    const out = [];
    const seenUrl = new Set();
    const seenId = new Set();

    // 1) curated seed first (highest priority; keeps its slugs and wiring).
    for (const s of curated) {
        seenUrl.add(s.url.trim());
        seenId.add(s.id);
        out.push(s);
    }

    // Dedupe + pick up to `cap` stations from a raw Radio Browser list and add
    // them to `out`. `fixedCountry` pins the country (a per-country pull);
    // otherwise it comes from the station and must be one we know (CAPS).
    // `forceGenre` overrides the tag classifier — used by tag pulls, where the
    // pulled tag already says what the station is. Returns how many were added.
    function absorb(arr, { cap, fixedCountry, forceGenre }) {
        const seenNorm = new Map();
        const picked = [];
        for (const s of arr) {
            const name = (s.name || '').trim();
            const url = (s.url_resolved || s.url || '').trim();
            const country = fixedCountry ?? (s.countrycode || '').toUpperCase();
            if (!name || !url || !/^https?:\/\//i.test(url) || seenUrl.has(url)) continue;
            if (!CAPS[country]) continue; // keep the catalog within known countries
            const nn = normName(name);
            if (!nn) continue;
            if (seenNorm.has(nn)) {
                const prev = picked[seenNorm.get(nn)];
                prev._tags += ',' + (s.tags || '');
                if ((s.bitrate || 0) > (prev.kbps || 0)) {
                    seenUrl.delete(prev.url);
                    seenUrl.add(url);
                    prev.name = name; prev.url = url; prev.kbps = s.bitrate || 128;
                }
                continue;
            }
            seenUrl.add(url);
            seenNorm.set(nn, picked.length);
            picked.push({ id: s.stationuuid, name, country, kbps: s.bitrate || 128, url, _tags: s.tags || '' });
            if (picked.length >= cap) break;
        }
        let added = 0;
        for (const rec of picked) {
            if (seenId.has(rec.id)) continue;
            seenId.add(rec.id);
            out.push({
                id: rec.id,
                name: rec.name,
                country: rec.country,
                genreKey: forceGenre ?? classifyGenre(rec._tags, rec.name),
                kbps: sanitizeKbps(rec.kbps),
                url: rec.url,
                gradIndex: grad(rec.id),
            });
            added++;
        }
        return added;
    }

    // 2) fresh Radio Browser pulls per country.
    for (const cc of Object.keys(CAPS)) {
        const n = absorb(await fetchCountry(cc), { cap: CAPS[cc], fixedCountry: cc });
        process.stderr.write(`${cc}:${n} `);
        await sleep(400); // be polite to the mirror
    }

    // 3) tag-targeted pulls to deepen genres the per-country lists leave thin.
    for (const { tag, genre, cap } of GENRE_PULLS) {
        const n = absorb(await fetchTag(tag), { cap, forceGenre: genre });
        process.stderr.write(`${tag}:${n} `);
        await sleep(400);
    }

    writeFileSync(OUT, JSON.stringify(out, null, 2) + '\n');

    const byCountry = {}, byGenre = {};
    for (const s of out) {
        byCountry[s.country] = (byCountry[s.country] || 0) + 1;
        byGenre[s.genreKey] = (byGenre[s.genreKey] || 0) + 1;
    }
    process.stderr.write('\n');
    console.log('TOTAL', out.length);
    console.log('COUNTRIES', Object.entries(byCountry).sort((a, b) => b[1] - a[1]).map(([k, v]) => `${k}:${v}`).join(' '));
    console.log('GENRES', Object.entries(byGenre).sort((a, b) => b[1] - a[1]).map(([k, v]) => `${k.replace('radio.genre.', '')}:${v}`).join(' '));
    console.log('wrote', OUT);
}

main().catch((e) => { console.error(e); process.exit(1); });
