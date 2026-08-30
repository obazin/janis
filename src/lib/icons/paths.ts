// Central SVG path data (24×24 viewBox unless noted). Components render
// these through `IconButton` / `NavItem` rather than embedding markup —
// one source for each glyph.

export const ICONS = {
    nowPlaying: 'M3 12h2l2-7 3 15 3-11 2 6h4',
    library: 'M4 5h5v14H4zM11 5h5v14h-5zM18 6l3 12',
    discover: 'M12 3a9 9 0 100 18 9 9 0 000-18zM15 9l-2 4-4 2 2-4z',
    radio: 'M5 6a10 10 0 000 12M19 6a10 10 0 010 12M8 9a5 5 0 000 6M16 9a5 5 0 010 6M12 11.5a.5.5 0 100 1 .5.5 0 000-1z',
    localFiles: 'M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z',
    spotify: 'M12 3a9 9 0 100 18 9 9 0 000-18zM7 10c3-1 7-1 10 1M8 15c2-.6 4-.4 6 .8',
    spotifyFull:
        'M12 2a10 10 0 100 20 10 10 0 000-20zM7 9c3-1 7-1 10 1M7.5 12.5c2.5-.8 5.5-.6 8 1M8 15.5c2-.6 4-.4 6 .8',
    equalizer: 'M4 8h10M18 8h2M4 16h2M10 16h10M14 6v4M8 14v4',
    search: 'M11 4a7 7 0 100 14 7 7 0 000-14zM21 21l-4-4',
    shuffle: 'M16 3h5v5M4 20L21 3M21 16v5h-5M15 15l6 6M4 4l5 5',
    repeat: 'M17 2l4 4-4 4M3 11V9a4 4 0 014-4h14M7 22l-4-4 4-4M21 13v2a4 4 0 01-4 4H3',
    // Filled transport glyphs.
    previous: 'M6 6h2v12H6zM20 6v12l-9-6z',
    next: 'M16 6h2v12h-2zM4 6l9 6-9 6z',
    play: 'M8 5v14l11-7z',
    pause: 'M6 5h4v14H6zM14 5h4v14h-4z',
    volume: 'M11 5L6 9H2v6h4l5 4zM15.5 8.5a5 5 0 010 7M19 5a9 9 0 010 14',
    plus: 'M12 5v14M5 12h14',
    upload: 'M12 16V4M7 9l5-5 5 5M4 16v2a2 2 0 002 2h12a2 2 0 002-2v-2',
    close: 'M6 6l12 12M18 6L6 18',
    minimize: 'M5 12h14',
    maximize: 'M6 6h12v12H6z',
    folder: 'M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z',
    refresh: 'M21 12a9 9 0 11-2.64-6.36M21 3v6h-6',
    alert: 'M12 4l9 15H3zM12 10v4M12 16.5h.01',
    imageOff: 'M4 5h16v14H4zM4 15l4-4 3 3 4-4 5 5M4 4l16 16',
} as const;

export type IconName = keyof typeof ICONS;
