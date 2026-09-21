# 0040. Markdown is rendered in the browser from text; no server-side HTML

Status: accepted.

## Context

Assistant messages, task descriptions, task comments and hand-off comments are markdown that an agent or a person wrote. The frontend showed only a fraction of it: `MarkdownBody` passed `react-markdown` its defaults, so a table stayed a wall of pipes, a task list stayed square brackets and a heading stayed hashes. Something had to render the rest.

Rendering it in the orchestrator with the Rust crate `pulldown_cmark` was the original idea, and it is rejected on three counts.

It would produce an HTML string. Putting that string on the page means `dangerouslySetInnerHTML`, and the text is untrusted: agent output is stored and displayed unredacted (ADR 0027), so the orchestrator would then have to sanitise it, adding a sanitiser to the backend to undo a problem the frontend does not have — `MarkdownBody` deliberately has no `rehype-raw`, so HTML in the source is inert text and no element is ever built from it.

It would need somewhere to put the HTML: a second field on every message, comment and description, or an endpoint beside each one, doubling the payload of a transcript page for a rendering the browser can do.

And it has no answer for streaming. Assistant text arrives as `text_delta` events that the browser assembles into a message (`SPEC.md`, "Session store"); the orchestrator never holds the finished text at the moment the reader sees it. Shipping the crate as WebAssembly instead would move the same renderer into the browser at the cost of a payload and a build step, and still leave the sanitising problem.

## Decision

Markdown is rendered in the browser, from the plain text the API already sends. `react-markdown` with `remark-gfm` is the renderer; `MarkdownBody` styles every element and stays the single place any written text is set. No HTML crosses the API, the orchestrator has no markdown dependency, and streaming text renders as it grows.

## Consequences

The sanitising question never arises: the pipeline builds elements from the markdown syntax tree, and raw HTML in the text is shown as text. Two element types are overridden for reasons of their own — `img` renders as a link carrying the alt text, so no remote fetch reports the viewer's address and session timing to whoever an agent was induced to name, and links open in a new tab through `react-markdown`'s default `urlTransform`, which drops `javascript:` and similar schemes.

The cost is a browser dependency and the styling of every element in one component. A non-browser consumer of the API — an export, a future email — gets the markdown text and renders it itself.
