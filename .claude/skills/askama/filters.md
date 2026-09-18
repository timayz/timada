# Askama Built-In Filters (0.16.0)

Source: <https://askama.rs/en/stable/filters.html>

Filters apply with `|` and chain left-to-right. Arguments go in parens: `value | filter(arg1, arg2)`.

Filters that take optional arguments also accept them by name, in any order, after all positional arguments: `dog{{ count | pluralize(plural = "gies") }}`.

Most string filters need the `alloc` feature (on by default); `unique` needs `std`, `urlencode`/`urlencode_strict` need `urlencode`, `json`/`tojson` need `serde_json`.

## String

| Filter | Effect | Example |
|--------|--------|---------|
| `capitalize` | First char upper, rest lower | `"hello" \| capitalize` → `"Hello"` |
| `lower` / `lowercase` | All lowercase | `"HI" \| lower` → `"hi"` |
| `upper` / `uppercase` | All uppercase | `"hi" \| upper` → `"HI"` |
| `title` / `titlecase` | Capitalize each word | `"hello WORLD" \| title` → `"Hello World"` |
| `trim` | Strip leading/trailing whitespace | `" hi " \| trim` → `"hi"` |
| `truncate(len)` | Cap length, append `…` if cut | `"hello" \| truncate(2)` → `"he..."` |
| `center(width)` | Pad with spaces, centered | `"a" \| center(5)` → `"  a  "` |
| `indent(width, [first], [blank])` | Indent each line; `width` may be a string to indent *with*. `first`/`blank` (both `false`) also indent the first and blank lines | `"a\nb" \| indent(4)` → `"a\n    b"` ; `"a\n\nb" \| indent("$ ", true, true)` |
| `wordcount` | Count words | `"a b c" \| wordcount` → `3` |

## Escaping & safety

| Filter | Effect | Example |
|--------|--------|---------|
| `escape` / `e` | HTML-escape `<`, `>`, `&`, `"`, `'` | `"<a>" \| e` → `"&lt;a&gt;"` |
| `escape("html")` | Force a specific escaper | overrides auto-escape choice |
| `safe` | Mark already-safe; skip escaping | `"<p>" \| safe` → `<p>` |
| `urlencode` | Percent-encode reserved chars | `"a?b" \| urlencode` → `"a%3Fb"` |
| `urlencode_strict` | Like `urlencode` but also escapes `/` | |

Never apply `safe` to user-controlled input.

## HTML content shaping

| Filter | Effect |
|--------|--------|
| `linebreaks` | `\n` → `<br />`, blank lines → `<p>` wrap |
| `linebreaksbr` | every `\n` → `<br />`, no `<p>` wrap |
| `paragraphbreaks` | blank lines → `<p>` wrap; single `\n` kept verbatim |

```jinja
{{ "hello\nworld\n\nfrom\naskama" | linebreaks }}
{# → <p>hello<br />world</p><p>from<br />askama</p> #}
```

## Numeric & formatting

| Filter | Effect | Example |
|--------|--------|---------|
| `filesizeformat([precision])` | Human-readable size | `1024 \| filesizeformat` → `"1.02 KB"` ; `1024 \| filesizeformat(precision = 3)` → `"1.024 KB"` |
| `format(args…)` | First arg is the format string | `"{:?}" \| format(x)` |
| `fmt("…")` | Chain-friendly variant | `x \| fmt("{:?}")` |
| `pluralize([sing="", plur="s"])` | Pick form from count | `2 \| pluralize` → `"s"` ; `1 \| pluralize("mouse","mice")` → `"mouse"` |

`format` takes the format string as the *piped* value; `fmt` takes the format string as an *argument*. Prefer `fmt` when composing in a chain.

## Collections

| Filter | Effect | Example |
|--------|--------|---------|
| `join(sep)` | Concatenate with separator | `["a","b"] \| join(", ")` → `"a, b"` |
| `unique` | Iterator without duplicates (needs `std`) | `["a","b","a"] \| unique` → `["a","b"]` |
| `reject(value)` | Drop items equal to `value` | `[1,2,3,1] \| reject(1)` → `[2,3]` |
| `reject(path)` | Drop items where the predicate holds; `path` is a function path taking `&&T -> bool` | `data \| reject(crate::is_odd)` |

## Fallbacks

| Filter | When fallback fires |
|--------|--------------------|
| `assigned_or(fallback)` | Value is in default state — `""`, `0`, `None`, `Err` |
| `defined_or(fallback)` | Left side is an *undefined* identifier (compile-time check) |
| `default(value, [boolean])` | Jinja-compat hybrid: behaves like `defined_or` unless the second argument is `true`, then like `assigned_or`. Prefer the two above |

```jinja
{{ user.name | assigned_or("anonymous") }}
{{ maybe_var | defined_or("fallback") }}
```

`defined_or`'s left side must be a bare identifier — it's resolved at compile time. `assigned_or` accepts any expression, but when given an identifier it checks definedness first.

## References

| Filter | Effect |
|--------|--------|
| `ref` | `x \| ref` ≡ `&x` |
| `deref` | `x \| deref` ≡ `*x` |

Useful inside filter chains where prefix `&`/`*` would be awkward.

## Feature-gated

### `json` / `tojson` — needs the `serde_json` feature

Serializes any `Serialize` value. Output never contains `&`, `<`, `>` or `'`. Compact by default; pass an integer for that many indent spaces, or a string to use as the indent prefix.

```jinja
Good: <li data-extra="{{ data | json }}">…</li>
Good: <li data-extra='{{ data | json | safe }}'>…</li>
Good: <pre>{{ data | json | safe }}</pre>
Good: <script>var data = {{ data | json | safe }};</script>

Bad:  <li data-extra="{{ data | json | safe }}">…</li>
Bad:  <script>var data = {{ data | json }};</script>
Bad:  <script>var data = "{{ data | json | safe }}";</script>
```

```jinja
<textarea>{{ data | tojson(4) }}</textarea>
<p>{{ data | tojson("\u{a0}\u{a0}") }}</p>
```

Rule of thumb: double-quoted attributes take `| json` alone; apostrophe-quoted attributes, HTML text and `<script>` bodies take `| json | safe`.

## Custom filters

**Since 0.15 every filter function must carry `#[askama::filter_fn]`** — this is the 0.14 → 0.15 breaking change. To be callable as a bare `| name`, the function must be reachable as `filters::name` from the template struct's scope, so put it in a `mod filters` next to the struct (a `filters` *crate* added as a dependency works too):

```rust
mod filters {
    #[askama::filter_fn]
    pub fn shout(s: impl std::fmt::Display, _env: &dyn askama::Values) -> askama::Result<String> {
        Ok(format!("{s}").to_uppercase())
    }
}
```

Anything defined elsewhere is called by path: `{{ name | some_module::shout }}`. `{{ v | my_filter }}` and `{{ v | filters::my_filter }}` are equivalent. Built-in filters take precedence over same-named custom ones — call yours by full path to escape the shadowing.

Signature requirements:

1. First parameter — the piped value. Prefer a trait bound (`impl Display`, `impl ToString`) over a concrete `&str`: askama's generated code hands over values at varying reference depths, and `Display` is implemented for `&str`, `&&str`, … so you avoid `{{ **value | filter }}` at call sites.
2. Second parameter — `env: &dyn askama::Values` (runtime environment). **Mandatory**, even when unused: omitting it fails to compile with *"Filter function missing required environment argument. Example: `fn filter0(_: &dyn std::fmt::Display, _: &dyn askama::Values) -> askama::Result<String>`"*. Name it `_env` if you don't read it.
3. Then any number of required arguments, then optional ones.
4. Return — `askama::Result<T>`. Only the *last* filter in a chain needs `T: Display`; intermediate ones may return anything.

Optional arguments are annotated `#[optional(default)]` and must come after the required ones:

```rust
#[askama::filter_fn]
pub fn example(
    value: impl Display,
    env: &dyn askama::Values,
    required0: impl Display,
    #[optional(None)] optional0: Option<&str>,
    #[optional("I am the default")] optional1: &str,
) -> askama::Result<String> { /* … */ }
```

`filter_fn` also enables named arguments at the call site — `{{ v | example("req0", optional1 = "x") }}` — though the compile errors for misuse are far less readable than for built-ins.

### Marking custom output HTML-safe

Either:

- Have the return type implement `askama::filters::HtmlSafe` (`impl askama::filters::HtmlSafe for MyStruct {}`, which covers `&MyStruct` too), or
- Return `askama::filters::Safe<T>` (always safe), or `askama::filters::MaybeSafe<T>` when only some inputs produce safe output — its variants are `MaybeSafe::Safe(v)` and `MaybeSafe::NeedsEscaping(v)`.

This avoids needing `| safe` at every call site. All primitive integer types are `HtmlSafe` already.

### Reading runtime values in a filter

```rust
#[askama::filter_fn]
pub fn cased(value: impl ToString, values: &dyn askama::Values) -> askama::Result<String> {
    let case: Option<Case> = askama::get_value(values, "case").ok();
    // …
}
```

See [`runtime.md`](./runtime.md).
