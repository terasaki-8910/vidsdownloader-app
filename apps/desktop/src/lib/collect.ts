/**
 * The reserved group shape for a page's own images (not inside links to other pages). The
 * collector script and the Rust shell use the same text; it can be chosen with
 * collect_select and saved in a rule like any link shape.
 */
export const IMAGES_SHAPE = "{images}";

/** Streams observed in the visible page, supplied by Rust rather than the page script. */
export const PLAYED_SHAPE = "__played__";
