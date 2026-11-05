// Which OS the webview runs on, for wording and behaviour that differ per OS. The same
// user-agent check AdBlockSection uses for "Windows".
const agent = typeof navigator === "undefined" ? "" : navigator.userAgent;

export const IS_WINDOWS = agent.includes("Windows");
export const IS_MAC = agent.includes("Macintosh") || agent.includes("Mac OS X");
