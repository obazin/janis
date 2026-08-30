/** Platform probe for chrome that differs around macOS window controls. */
export function isMac(): boolean {
    return typeof navigator !== 'undefined' && navigator.userAgent.includes('Macintosh');
}
