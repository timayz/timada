# Creating Askama Templates (0.16.0)

Source: <https://askama.rs/en/stable/creating_templates.html> · Config: <https://askama.rs/en/stable/configuration.html>

A template is a Rust struct (or enum) decorated with `#[derive(Template)]` and a `#[template(...)]` attribute. Struct fields become the template's variables; Askama generates the rendering code at compile time.

## Minimal example

```rust
use askama::Template;

#[derive(Template)]
#[template(path = "hello.html")]
struct HelloTemplate<'a> {
    name: &'a str,
}

let s = HelloTemplate { name: "World" }.render()?;
```

Template files live in `templates/` at the crate root by default.

## `#[template(...)]` attributes

| Attribute | Purpose |
|-----------|---------|
| `path = "file.html"` | File under `templates/`. Extension drives auto-escape and MIME. Mutually exclusive with `source`. |
| `source = "…"` | Inline template body. Requires `ext`. Mutually exclusive with `path`. |
| `ext = "html"` | File extension for `source`; controls escape mode and content type. Mutually exclusive with `path`. |
| `escape = "html"` \| `"none"` \| … | Override the extension-derived escaper. |
| `print = "none"` \| `"ast"` \| `"code"` \| `"all"` | Compile-time debug output of parsed AST or generated code. |
| `block = "name"` | Render a single named block; only that block's variables are required on the struct. |
| `blocks = ["title", "content"]` | Generate sub-templates that behave as if they had `block = "…"`; access via `my.as_title()` / `my.as_content()`. |
| `in_doc = true` | Read template from a fenced ```` ```askama ```` block in the struct's doc comment (needs `code-in-doc` feature). Combine with `ext`. |
| `syntax = "custom"` | Use a custom-named syntax defined in the config file. |
| `config = "path.toml"` | Config file path relative to crate root. |
| `whitespace = "suppress"` \| … | Default whitespace handling. |
| `askama = $crate::__askama` | Override the path to the `askama` crate (for re-exports in libraries/macros). |

## Field → variable mapping

```rust
#[derive(Template)]
#[template(source = "{{ name }} is {{ age }}", ext = "txt")]
struct Person<'a> {
    name: &'a str,
    age: u32,
}
```

Lifetimes and generics on the struct are preserved. Visibility (`pub`, `pub(crate)`) does not affect template access; the template sees all fields.

## Enums as templates

Each variant can share one template or get its own:

```rust
#[derive(Template)]
#[template(path = "area.txt")]
enum Area {
    Square(f32),
    Rectangle { a: f32, b: f32 },
    Circle { radius: f32 },
}
```

In `area.txt`, dispatch with `{% match self %}` and `{% when Self::Square(side) %}` / `{% when Self::Rectangle { a, b } %}`.

Per-variant templates:

```rust
#[derive(Template)]
#[template(ext = "txt")]
enum AreaPerVariant {
    #[template(source = "{{ self.0 }}^2")]
    Square(f32),
    #[template(source = "{{ a }} * {{ b }}")]
    Rectangle { a: f32, b: f32 },
}
```

Variants inherit `config`, `escape`, `ext`, `syntax`, and `whitespace` from the enum's attribute, but not `block` or `print`. Any variant left un-annotated falls back to the enum's own template, which must therefore exist.

Alternatively, use `block = "…"` on each variant to point at named blocks in a shared file.

## `in_doc` templates

```rust
/// ```askama
/// <div>{{ content }}</div>
/// ```
#[derive(Template)]
#[template(ext = "html", in_doc = true)]
struct Example<'a> {
    content: &'a str,
}
```

`jinja` / `jinja2` are accepted in place of `askama` for editor syntax highlighting.

## Rendering

```rust
let out: String = template.render()?;
```

`.render()` returns `askama::Result<String>`. There are no framework integration crates any more (`askama_axum` & co. were dropped in 0.13, along with `Template::EXTENSION` / `Template::MIME_TYPE`) — render to a `String` and build the response yourself, setting `Content-Type` explicitly. `err.into_io_error()` / `err.into_box()` convert the error where a framework wants `std::io::Error` or a boxed error.

`render()` needs the `alloc` feature; `write_into()` needs `std`.

## Configuration (`askama.toml`)

Read at compile time from the crate root when the `config` feature is on (default). The defaults:

```toml
[general]
dirs = ["templates"]
whitespace = "preserve"   # or "suppress" / "minimize"
```

`dirs` entries support globs — `["templates/*"]`, or `["templates/**"]` to recurse into sub-folders.

Custom syntaxes and escapers are also declared here:

```toml
[general]
default_syntax = "foo"

[[syntax]]
name = "foo"            # block_start/block_end/comment_start/comment_end/
block_start = "%{"      # expr_start/expr_end; omitted keys keep the default,
expr_end = "^^"         # values must be ≥ 2 characters

[[escaper]]
path = "askama::filters::Text"   # any type implementing `Escaper`
extensions = ["js"]              # matched before the built-in escapers
```

Built-in escapers cover HTML (`html`, `htm`, `xml`, `j2`, `jinja`, `jinja2`) and text/no-op (`md`, `yml`, `none`, `txt`, and the empty extension). A configured escaper can also be invoked by name: `{{ s | escape("tex") }}`.

Point a template at a non-default file with `#[template(config = "other.toml")]`.

## Choosing `path` vs `source`

- `path` — the normal case. Keeps templates in `templates/`, where editors highlight them and they're easy to diff.
- `source` — short/embedded snippets, tests, or programmatic templates. Requires `ext`.

## Debugging

Set `print = "code"` to see the generated rendering code at compile time — useful when the template compiles but produces unexpected output.
