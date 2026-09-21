# Kitchen sink

Every construct `MarkdownBody` renders, in one file: the unit tests read it and
it is the thing to look at when the styling changes.

## Headings

### Level three

#### Level four

##### Level five

###### Level six

## Inline

**Bold**, *italic*, ~~struck through~~, `inline_code()`, a [link](https://example.invalid/docs),
an autolink https://example.invalid/autolink and a footnote reference[^note].

Text that only looks like markdown: snake_case_names stay literal, and so does
2 * 3 * 4.

A path with no break opportunity: /very/long/path/that/keeps/going/without/a/single/space/segment/and/then/some/more.

## Lists

- first
- second
  - nested
    - deeper
- third

1. one
2. two
   1. two point one
3. three

- [x] a finished item
- [ ] an unfinished item

## Quote

> A quote, muted and set off by a rule.
>
> ```sh
> echo "a fence inside a quote"
> ```

## Rule

---

## Table

| Left | Centre | Right |
| :--- | :----: | ----: |
| `a` | b | 1 |
| a cell that is very much longer than the others and keeps going for a while | c | 22 |

A table inside a list item:

- the item
  | k | v |
  | - | - |
  | a | 1 |

## Code

```rust
fn main() {
    println!("hello");
}
```

## Not rendered

<script>alert("no")</script>

<img src="https://example.invalid/pixel.png" onerror="alert(1)">

![an image](https://example.invalid/pixel.png)

[a dangerous link](javascript:alert(1))

[^note]: The footnote body.
