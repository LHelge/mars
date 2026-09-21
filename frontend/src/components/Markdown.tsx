// Markdown prose, wherever the application shows text a human or an agent
// wrote: assistant messages (`SPEC.md`, "Transcript rendering"), task
// descriptions, task comments and hand-off comments.
//
// GitHub-flavoured markdown through `remark-gfm` (tables, strikethrough, task
// lists, autolinks, footnotes), rendered in the browser from the text the API
// already sends (ADR 0040). Every element is styled here, with the console
// tokens only: the hierarchy is carried by weight and spacing, not by size and
// not by colour, because colour on this UI means state.
//
// Two things are deliberately not rendered:
//
//   * No `rehype-raw`: `react-markdown`'s default never turns HTML in the text
//     into elements, and both agent output and user text are untrusted text
//     that happens to be displayed unredacted (ADR 0027).
//   * No images. `img` becomes a link carrying the alt text, because fetching
//     a remote image from agent-written text would tell whoever the agent was
//     induced to name the viewer's address and the timing of the session.
//
// `react-markdown`'s default `urlTransform` stays in place; it is what drops
// `javascript:` and other dangerous schemes from links.

import type { Element } from "hast";
import type { ComponentProps } from "react";
import { createContext, use } from "react";
import Markdown from "react-markdown";
import type { Components, ExtraProps } from "react-markdown";
import remarkGfm from "remark-gfm";

// By path, not through the barrel: `CodeBlock` reaches the highlighter with a
// dynamic import, and the barrel is in the entry chunk (`SPEC.md`, "Frontend",
// "Code splitting").
import { CodeBlock } from "./CodeBlock";

// Block code is `pre > code`; react-markdown 10 no longer tells the `code`
// component whether it is inline, so `pre` says so for it. The provider wraps
// the children `pre` was handed, which are rendered below it.
const InPre = createContext(false);

// Nested lists change marker rather than indent step: one glance says how deep
// a bullet is without the text marching further right at every level.
const ListDepth = createContext(0);

const UL_MARKERS = ["list-disc", "list-[circle]", "list-[square]"];
const OL_MARKERS = ["list-decimal", "list-[lower-alpha]", "list-[lower-roman]"];

// The measure cap sits on the text blocks, not on the component's root, so a
// table or a code fence may use the full width of the row it is in.
const PROSE = "max-w-prose";

type Props<T extends keyof React.JSX.IntrinsicElements> = ComponentProps<T> &
  ExtraProps;

function cx(...parts: (string | undefined | false)[]): string {
  return parts.filter(Boolean).join(" ");
}

// `react-markdown` hands each component the hast node beside the element's own
// attributes. The node is not a DOM attribute, so it is dropped before the
// rest are passed on; everything else the source carried — an id, a footnote's
// data attribute, a generated class — is kept.
function attrs<P extends ExtraProps>(props: P): Omit<P, "node"> {
  const rest = { ...props };
  delete rest.node;
  return rest;
}

// The generated classes that matter: `mdast-util-to-hast` marks a task list on
// the `ul` and each of its items.
function hasClass(className: string | undefined, name: string): boolean {
  return (className ?? "").split(/\s+/).includes(name);
}

function MdA(props: Props<"a">) {
  const { href, className } = props;
  // A same-document anchor — a footnote reference or its back-link — stays in
  // the document: opening it in a new tab would lose the transcript, and it
  // must not take the single-page application anywhere either.
  const internal = href !== undefined && href.startsWith("#");
  return (
    <a
      {...attrs(props)}
      {...(internal ? {} : { target: "_blank", rel: "noopener noreferrer" })}
      className={cx(
        "text-console-accent underline underline-offset-2",
        className,
      )}
    />
  );
}

// Not loaded: the alt text and the address, as a link the reader may choose to
// follow.
function MdImg({ src, alt }: Props<"img">) {
  return (
    <a
      href={typeof src === "string" ? src : undefined}
      target="_blank"
      rel="noopener noreferrer"
      className="text-console-accent font-mono text-[0.85em] underline underline-offset-2"
    >
      {`[image: ${alt !== undefined && alt !== "" ? alt : "no description"}]`}
    </a>
  );
}

function MdCode(props: Props<"code">) {
  const inPre = use(InPre);
  return (
    <code
      {...attrs(props)}
      className={cx(
        "font-mono text-[0.85em]",
        // An inline token keeps its own background and is never broken in the
        // middle to make a line fit; a fence is styled by its `pre`.
        !inPre &&
          "bg-console-raised border-console-border/60 rounded border px-1 py-px [overflow-wrap:normal]",
        props.className,
      )}
    />
  );
}

// What a fenced block is made of, read off the syntax tree rather than off the
// rendered children: the code has to reach `Copy` as the text the writer
// fenced, not as a walk of the DOM. `mdast-util-to-hast` closes every code
// node with a newline of its own; that one is the fence, not the code, so it
// is dropped and never copied.
interface Fence {
  code: string;
  language?: string;
}

function fenceOf(node: Element | undefined): Fence | null {
  const code = node?.children.find(
    (child) => child.type === "element" && child.tagName === "code",
  );
  if (code === undefined || code.type !== "element") return null;
  if (code.children.some((child) => child.type !== "text")) return null;

  let text = code.children
    .map((child) => (child.type === "text" ? child.value : ""))
    .join("");
  if (text.endsWith("\n")) text = text.slice(0, -1);

  const classes = code.properties.className;
  const tag = (Array.isArray(classes) ? classes : [])
    .map(String)
    .find((name) => name.startsWith("language-"))
    ?.slice("language-".length);

  return { code: text, language: tag === "" ? undefined : tag };
}

function MdPre(props: Props<"pre">) {
  const fence = fenceOf(props.node);
  if (fence !== null) {
    return <CodeBlock code={fence.code} language={fence.language} />;
  }
  // Anything else that arrived as a `pre` — there is no `rehype-raw`, so this
  // is rare — keeps the plain bordered box.
  return (
    <pre
      {...attrs(props)}
      className={cx(
        "border-console-border bg-console-bg my-2 overflow-x-auto rounded border px-3 py-2 font-mono text-xs [overflow-wrap:normal]",
        props.className,
      )}
    >
      <InPre value={true}>{props.children}</InPre>
    </pre>
  );
}

function MdUl(props: Props<"ul">) {
  const depth = use(ListDepth);
  // A task list carries its own markers; a bullet beside a checkbox is noise.
  const marker = hasClass(props.className, "contains-task-list")
    ? "list-none"
    : UL_MARKERS[depth % UL_MARKERS.length];
  return (
    <ul
      {...attrs(props)}
      className={cx("my-1 space-y-0.5 pl-5", PROSE, marker, props.className)}
    >
      <ListDepth value={depth + 1}>{props.children}</ListDepth>
    </ul>
  );
}

function MdOl(props: Props<"ol">) {
  const depth = use(ListDepth);
  return (
    <ol
      {...attrs(props)}
      className={cx(
        "my-1 space-y-0.5 pl-5",
        PROSE,
        OL_MARKERS[depth % OL_MARKERS.length],
        props.className,
      )}
    >
      <ListDepth value={depth + 1}>{props.children}</ListDepth>
    </ol>
  );
}

function MdLi(props: Props<"li">) {
  return (
    <li
      {...attrs(props)}
      className={cx(
        hasClass(props.className, "task-list-item") && "list-none",
        props.className,
      )}
    />
  );
}

// The task-list checkbox: read-only evidence of what the writer ticked.
function MdInput({ checked, type }: Props<"input">) {
  if (type !== "checkbox") {
    return null;
  }
  return (
    <input
      type="checkbox"
      checked={checked ?? false}
      disabled
      readOnly
      className="accent-console-accent mr-1.5 align-middle"
    />
  );
}

function MdP(props: Props<"p">) {
  return (
    <p {...attrs(props)} className={cx("my-1", PROSE, props.className)} />
  );
}

// A compact heading scale: this is a console, not an article, so h1 and h2 are
// barely larger than the body and weight and spacing do the work.
type HeadingTag = "h1" | "h2" | "h3" | "h4" | "h5" | "h6";

function heading(Tag: HeadingTag, own: string) {
  // Every heading level takes the same props; only the tag and the class
  // differ, so one factory spares six near-identical components.
  function Heading(props: Props<"h1">) {
    return (
      <Tag {...attrs(props)} className={cx(own, PROSE, props.className)} />
    );
  }
  Heading.displayName = `Md${Tag.toUpperCase()}`;
  return Heading;
}

const MdH1 = heading("h1", "mt-3 mb-1 text-[1.15em] font-semibold first:mt-0");
const MdH2 = heading("h2", "mt-3 mb-1 text-[1.075em] font-semibold first:mt-0");
const MdH3 = heading("h3", "mt-2 mb-1 font-semibold first:mt-0");
const MdH4 = heading("h4", "mt-2 mb-1 font-semibold first:mt-0");
const MdH5 = heading(
  "h5",
  "text-console-muted mt-2 mb-1 font-semibold first:mt-0",
);
const MdH6 = heading(
  "h6",
  "text-console-muted mt-2 mb-1 text-[0.9em] font-semibold tracking-wide uppercase first:mt-0",
);

function MdBlockquote(props: Props<"blockquote">) {
  return (
    <blockquote
      {...attrs(props)}
      className={cx(
        "border-console-border text-console-muted my-2 border-l-2 pl-3",
        PROSE,
        props.className,
      )}
    />
  );
}

// The footnote section `remark-gfm` appends: set apart by a quiet rule, and
// the label it carries for screen readers stays hidden.
function MdSection(props: Props<"section">) {
  return (
    <section
      {...attrs(props)}
      className={cx(
        hasClass(props.className, "footnotes") &&
          "border-console-border text-console-muted mt-3 border-t pt-2 text-xs",
        props.className,
      )}
    />
  );
}

const COMPONENTS: Components = {
  a: MdA,
  img: MdImg,
  code: MdCode,
  pre: MdPre,
  ul: MdUl,
  ol: MdOl,
  li: MdLi,
  input: MdInput,
  p: MdP,
  h1: MdH1,
  h2: MdH2,
  h3: MdH3,
  h4: MdH4,
  h5: MdH5,
  h6: MdH6,
  blockquote: MdBlockquote,
  section: MdSection,
  hr: () => <hr className="border-console-border my-3" />,
  strong: ({ children }) => (
    <strong className="font-semibold">{children}</strong>
  ),
  em: ({ children }) => <em className="italic">{children}</em>,
  del: ({ children }) => (
    <del className="text-console-muted line-through">{children}</del>
  ),
  // A wide table scrolls inside its own box; it never widens the row it is in.
  table: ({ children }) => (
    <div className="my-2 max-w-full overflow-x-auto">
      <table className="border-console-border w-auto border-collapse text-xs">
        {children}
      </table>
    </div>
  ),
  thead: ({ children }) => (
    <thead className="bg-console-raised">{children}</thead>
  ),
  // GFM column alignment arrives as an inline style and is kept.
  th: ({ children, style }) => (
    <th
      style={style}
      className="border-console-border border px-2 py-1 text-left font-semibold"
    >
      {children}
    </th>
  ),
  td: ({ children, style }) => (
    <td
      style={style}
      className="border-console-border border px-2 py-1 align-top"
    >
      {children}
    </td>
  ),
};

const PLUGINS = [remarkGfm];

export interface MarkdownBodyProps {
  children: string;
  // Classes for the root, for a caller that renders one text as several
  // bodies: `StreamingMarkdown` uses it to say that a body continues the one
  // above it, so `first:mt-0` does not eat a heading's margin mid-message.
  className?: string;
}

export function MarkdownBody({ children, className }: MarkdownBodyProps) {
  return (
    // A URL, a hash or a path with no break opportunity wraps rather than
    // widening the row; `pre` and inline code opt back out.
    <div className={cx("[overflow-wrap:anywhere]", className)}>
      <Markdown components={COMPONENTS} remarkPlugins={PLUGINS}>
        {children}
      </Markdown>
    </div>
  );
}
