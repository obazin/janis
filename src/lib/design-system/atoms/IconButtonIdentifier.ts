// App-wide registry of icon-only buttons. Every `IconButton` instance names
// itself here — the registry is the searchable inventory of clickable icons.

export enum IconButtonIdentifier {
    TransportShuffle = 'transport-shuffle',
    TransportPrevious = 'transport-previous',
    TransportPlayPause = 'transport-play-pause',
    TransportNext = 'transport-next',
    TransportRepeat = 'transport-repeat',
    MiniPrevious = 'mini-previous',
    MiniPlayPause = 'mini-play-pause',
    MiniNext = 'mini-next',
    EqClose = 'eq-close',
    LocalRemoveFolder = 'local-remove-folder',
    LocalRescan = 'local-rescan',
    WindowMinimize = 'window-minimize',
    WindowMaximize = 'window-maximize',
    WindowClose = 'window-close',
}
