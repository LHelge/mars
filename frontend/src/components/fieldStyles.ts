// The console's control styling, in one place (`CLAUDE.md`, "Frontend
// conventions": shared UI).
//
// Every input, select and textarea in the app is the same bordered, monospace
// control; it used to be a class string copied into fifteen files and four
// differently-drifted constants. Both of these are plain strings, so a control
// that genuinely needs more — a width cap, a raised background — appends to
// one of them rather than starting a new copy.
//
// The touch half of the pointer rule lives here too (`SPEC.md`, "Frontend",
// "Mobile layout"): on a coarse pointer every text control is 16 px, so a
// phone does not zoom into the field it focuses, and every control has a hit
// area of at least 44 px. Every class of that rule is behind `pointer-coarse:`,
// never `any-pointer-coarse:`, so a laptop with a touch screen and a trackpad
// keeps the console's density, and a fine pointer renders exactly what it did
// before the rule existed.

/**
 * A text control's size on a coarse pointer: 16 px, the size below which iOS
 * zooms the page on focus. `CONTROL` carries it; a deliberately compact
 * control with a class string of its own (a `text-xs` inline select, the
 * composer) appends it, and the hint or label beside the control keeps its own
 * size.
 */
export const TOUCH_TEXT = "pointer-coarse:text-base";

/**
 * The control itself. `aria-invalid:border-state-failed` is why a control that
 * takes `FieldShell`'s `aria-invalid` turns red on an error without the caller
 * doing anything, and `placeholder:` is inert on a control with no placeholder.
 */
export const CONTROL = `border-console-border bg-console-bg text-console-text placeholder:text-console-muted aria-invalid:border-state-failed rounded border px-2.5 py-1.5 font-mono text-sm disabled:opacity-50 ${TOUCH_TEXT}`;

/** The same control where it fills the column it sits in. */
export const FIELD = `${CONTROL} w-full`;

/**
 * A 44 px hit area that is the element's own box: at least 44 px tall and wide
 * on a coarse pointer. For a button (which centres its content) or a flex or
 * inline-flex element that centres its items — `SubmitButton`, an icon-only
 * button, a nav link, a tab — where growing the box on a phone is the point.
 */
export const TAP = "pointer-coarse:min-h-11 pointer-coarse:min-w-11";

/**
 * A 44 px hit area that leaves the layout alone: on a coarse pointer an
 * absolutely positioned `::after`, centred on the element and at least 44 px
 * each way, takes the taps around it, while the element keeps its own box. For
 * what sits inside a line of text or a dense row — a chip button, an unpadded
 * `text-xs` link — where `TAP`'s `min-h-11` would change the line height of the
 * card or row it sits in. The element becomes the pseudo-element's containing
 * block through `pointer-coarse:relative`, so this is never for an element
 * that is itself `absolute`, `fixed` or `sticky` (use `TAP` there), and an
 * ancestor with `overflow` clips the area to itself. Neighbours closer than
 * 44 px share the overlap, the later one taking it.
 */
export const TAP_INLINE =
  "pointer-coarse:relative pointer-coarse:after:absolute pointer-coarse:after:top-1/2 pointer-coarse:after:left-1/2 pointer-coarse:after:size-full pointer-coarse:after:min-h-11 pointer-coarse:after:min-w-11 pointer-coarse:after:-translate-x-1/2 pointer-coarse:after:-translate-y-1/2";

/**
 * A checkbox's or radio's size on a coarse pointer: 20 px. `CHECK` carries it;
 * a box of another resting size appends it to its own class string.
 */
export const CHECK_TOUCH = "pointer-coarse:size-5";

/** Every checkbox and radio: the accent-coloured 14 px box, 20 px on touch. */
export const CHECK = `accent-console-accent size-3.5 ${CHECK_TOUCH}`;

/**
 * The `<label>` that wraps a checkbox or radio and is its hit area: 44 px tall
 * on a coarse pointer. The label is a flex row, so the box and its sentence
 * stay on one line and the whole row takes the tap.
 */
export const CHECK_LABEL = "pointer-coarse:min-h-11";
