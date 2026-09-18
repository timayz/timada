---
name: askama
description: Reference for Askama (Rust) template syntax at version 0.16.0 — use when writing, editing, or debugging .html/.j2/.jinja templates in Rust projects that use the `askama` crate, or when generating template strings via `#[template(source = "…")]`. Covers variables, `{% let %}` / `{% mut %}` / `{% decl %}` assignments and compound assignments, filters and filter blocks, control flow (`if` / `if let` / `is defined` / `for` / `match`), inheritance and block fragments, macros with default and named arguments plus `{% call %}` blocks and `caller()`, includes, `{% raw %}`, whitespace control, and HTML escaping. Also flags 0.15/0.16 breaking changes — custom filters now require `#[askama::filter_fn]`, a variable without a value needs `{% decl %}` instead of `{% let %}`, `caller` is a reserved variable name, duplicated block names are a compile error, MSRV is 1.88. Companion files in this skill directory: `filters.md` (full filter catalog), `creating-templates.md` (`#[derive(Template)]`, `#[template(...)]` attributes, `askama.toml`), `runtime.md` (runtime values API).
---

# Askama Template Syntax — 0.16.0

Askama is a type-safe, compile-time template engine for Rust with Jinja-like syntax. Templates are bound to a struct via `#[derive(Template)]` and `#[template(path = "…")]`.

Source: <https://askama.rs/en/stable/template_syntax.html> · Docs: <https://docs.rs/askama/0.16.0>

## Install

```toml
[dependencies]
askama = "0.16"
# Opt-in features, e.g. the `|json` filter and `in_doc = true` templates:
# askama = { version = "0.16", features = ["serde_json", "code-in-doc"] }
```

MSRV is 1.88. Default features: `config`, `derive`, `std` (implies `alloc`), `urlencode`. Additional: `serde_json` (`|json`), `code-in-doc` (`in_doc = true`), `nightly-spans`; `full` = default + `code-in-doc` + `serde_json`. Dropping `alloc`/`std` makes askama `no_std`-usable but removes `Template::render()` / `Template::write_into()` respectively.

Companion references in this directory:

- [`creating-templates.md`](./creating-templates.md) — `#[derive(Template)]`, all `#[template(...)]` attributes, enums as templates, `askama.toml`.
- [`filters.md`](./filters.md) — complete built-in filter catalog and custom-filter rules.
- [`runtime.md`](./runtime.md) — runtime values, `render_with_values`, `get_value`.

## Delimiters

| Delimiter | Purpose |
|-----------|---------|
| `{{ … }}` | Expression — renders a value |
| `{% … %}` | Statement — control flow, declarations |
| `{# … #}` | Comment (nestable) |
| `{% raw %}…{% endraw %}` | Print contents verbatim, no templating |

## Variables & expressions

- `{{ name }}` — field on the template context struct
- `{{ user.name }}` — nested field access
- `{{ crate::MAX_USERS }}` — Rust constants/paths
- Operators follow Rust precedence: `+ - * / %`, comparison, `&&`, `||`, parentheses for grouping
- Bit ops use word forms to avoid filter ambiguity: `bitand`, `bitor`, `xor`
- `as` cast works for primitive types only
- String concatenation: `{{ a ~ b ~ c }}` (spaces required around `~`)

## Assignments

```jinja
{% let name = user.name %}
{% let len = name.len() %}
{% let mut foo = [1, 2].iter() %}
```

Variables can shadow and be `mut`. `set` is also accepted (Jinja compatibility). Names may not start with `__askama`, be a Rust keyword, or be `caller` (reserved for `{% call %}` blocks since 0.15).

**Compound assignment** — `{% mut … %}` with any Rust augmented-assignment operator; the target must be mutable:

```jinja
{%- let mut counter = 0 -%}
{%- for i in 1..=10 -%}
  {%- mut counter += i -%}
  {{ counter }}
{% endfor -%}
```

**Block form** (initialize with a block-computed string):

```jinja
{% let x %}
  {{ crate::some_function() }} = {{ a * b }}
{% endlet %}
```

**Deferred declaration** — `decl` (or `declare`); since 0.16 a valueless `{% let %}` / `{% set %}` is an error, use `decl`:

```jinja
{% decl val -%}
{% if len == 0 %}{% let val = "foo" %}{% else %}{% let val = name %}{% endif %}
{{ val }}
```

**Borrow rules** — the initializer is put behind a reference only when it is a plain field access (`x.y`). Multi-element expressions (`x + 2`), template-local variables, filtered values (`x|capitalize`), and `?` expressions are *not* referenced.

## Filters

Chain with `|`; arguments in parens:

```jinja
{{ "{:?}"|format(name|escape) }}
{{ value | safe }}
{{ value | escape }}   {# or | e #}
```

**Filter block** applies filters to a span:

```jinja
{% filter lower|capitalize %}
  {{ text }}
{% endfilter %}
```

Filters with optional arguments also accept named arguments, in any order, after the positional ones: `{{ count | pluralize(plural = "gies") }}`.

Custom filters must carry `#[askama::filter_fn]` (since 0.15) and take `&dyn askama::Values` as a mandatory second parameter. A bare `{{ v | shout }}` resolves to `filters::shout`, so define them in a `mod filters` in the context's scope; anything else is called by path (`{{ v | my_mod::shout }}`). Built-in filters shadow same-named custom ones — call yours by path to avoid that.

For the complete list of built-in filters (string, escaping, collection, formatting, fallbacks, references, `json`/`tojson`) and rules for defining custom filters, see [`filters.md`](./filters.md).

## Control flow

### `if` / `else if` / `else`

```jinja
{% if users.is_empty() %}
  No users
{% else if users.len() == 1 %}
  1 user
{% elif users.len() == 2 %}
  2 users
{% else %}
  {{ users.len() }} users
{% endif %}
```

`elif` is an accepted spelling of `else if`.

**`if let`** for `Option`/`Result`/enum patterns:

```jinja
{% if let Some(u) = current_user %}{{ u.name }}{% endif %}
```

**Existence check** — usable in conditions *and* expressions:

```jinja
{% if var is defined %}…{% endif %}
{% if var is not defined %}…{% endif %}
{% if x is defined && x == "12" %}…{% endif %}
<script>const x = {{ x is defined }};</script>
```

Only the current type's fields and template-declared variables are visible to the proc macro, so `{% if x.y is defined %}` does not compile.

### `for`

```jinja
{% for item in items if item.active %}
  {{ loop.index }}. {{ item.name }}
{% else %}
  empty
{% endfor %}
```

Loop vars: `loop.index`, `loop.index0`, `loop.first`, `loop.last`. Body may use `{% break %}` and `{% continue %}`.

### `match`

```jinja
{% match result %}
  {% when Ok(v) %} ok: {{ v }}
  {% when Err(e) %} err: {{ e }}
{% endmatch %}
```

Patterns support literals, destructuring, alternatives, and wildcards:

```jinja
{% match n %}
  {% when 1 | 2 | 3 %} low
  {% when [first, ..] %} non-empty slice
  {% else %} other
{% endmatch %}
```

`{% when Some with (val) %}` is an accepted alternative to `{% when Some(val) %}`. A match must be exhaustive and needs at least one `{% when %}` or `{% else %}`; `{% else %}` is sugar for `{% when _ %}` and must come last. Only whitespace and comments may sit between `{% match %}` and the first `{% when %}`. Optional `{% endwhen %}` improves linter compatibility.

## Template inheritance

**Base** (`base.html`):

```jinja
<!DOCTYPE html>
<html>
  <head><title>{% block title %}Default{% endblock %}</title></head>
  <body>{% block content %}<p>placeholder</p>{% endblock %}</body>
</html>
```

**Child:**

```jinja
{% extends "base.html" %}

{% block title %}Page{% endblock %}

{% block content %}
  <h1>Hi</h1>
  {{ super() }}
{% endblock %}
```

- `super()` renders the parent block's content.
- Content outside blocks in the child template is ignored — so `{% extends %}` rejects whitespace control (`{%- extends "base.html" +%}` is an error).
- `endblock title` (named close) is allowed.
- Blocks may only appear at the top level or nested inside another block — not inside `if`/`else` branches or `for` bodies.
- Since 0.16 a duplicated block name is a compile error (it used to warn).

**Block fragments** — render one block independently from Rust:

```rust
#[derive(Template)]
#[template(path = "page.html", block = "content")]
struct ContentFragment { /* … */ }
```

## Includes

```jinja
{% for item in items %}
  {% include "item.html" %}
{% endfor %}
```

Path must be a string literal (resolved at compile time). Included templates see the calling context.

## Macros

```jinja
{% macro heading(title, subtitle = "default") %}
  <h1>{{ title }}</h1>
  <h2>{{ subtitle }}</h2>
{% endmacro %}

{{ heading("Title") }}
{{ heading("Title", "Sub") }}
{{ heading(title = "Title", subtitle = "Sub") }}
```

**Type annotations:**

```jinja
{% macro show(value: Option<u32>) %}
  {% if let Some(v) = value %}{{ v }}{% endif %}
{% endmacro %}
```

**Import from another file:**

```jinja
{% import "macros.html" as m %}
{{ m::heading("Title") }}
```

Macros inherit the variable scope of their call site. `{% endmacro heading %}` (named close) is allowed.

**Named arguments:** allowed in any order after positional args. Optional args (with defaults) come last in the definition. Naming an argument that a positional argument already filled is an error — `{{ heading("a", "b", arg2 = "x") }}` fails because `"b"` is `arg2`.

**Call blocks** — pass a body to a macro:

```jinja
{% macro centered() %}<center>{{ caller() }}</center>{% endmacro %}

{% call centered() %}Hello{% endcall %}
```

**Call block with arguments:**

```jinja
{% macro list_users(users) %}
  {% for u in users %}<li>{{ caller(u) }}</li>{% endfor %}
{% endmacro %}

{% call(user) list_users(users) %}
  Name: {{ user.name }}
{% endcall %}
```

Inside a macro, guard with `{% if caller is defined %}` to support both invocation styles — invoking a `caller()`-using macro as `{{ centered() }}` otherwise fails, because only `call` blocks define `caller`.

`caller()` can also be captured into a variable:

```jinja
{% macro test() %}{% set content = caller() %}-> `{{ content }}` <-{% endmacro %}
```

**Nesting call blocks** — an inner `{% call %}` overwrites `caller`, so alias it first:

```jinja
{% macro outer_container() %}
  {% set outer_caller = caller %}
  {% call container() %}{{ outer_caller() }}{% endcall %}
{% endmacro %}
```

## Functions, methods, closures

| Call site | Syntax |
|-----------|--------|
| Field (function-typed) | `{{ foo(arg) }}` |
| Free function in template module | `{{ self::function(arg) }}` |
| Public path | `{{ crate::module::function(arg) }}` |
| Method on `self` | `{{ self.method(arg) }}` or `{{ method(arg) }}` |
| Trait method | `{{ Self::method(arg) }}` |
| Closure | `{{ (closure)(arg) }}` |

A bare name is always read as a method on `self`; anything else needs a path (`self::`, `super::`, `crate::`).

## Struct instantiation

```jinja
{{ MyStruct { field1: 1, field2: "v" }.method() }}
{{ MyStruct { field1: 1, ..other } }}
{{ MyStruct { field1: 1, ..Default::default() } }}
```

## Whitespace control

Default: whitespace preserved except trailing newline of the file.

| Marker | Effect |
|--------|--------|
| `-`    | Suppress whitespace |
| `~`    | Minimize to a single character (newline if any) |
| `+`    | Preserve explicitly |

```jinja
{% if x %}
  {{- y -}}
{% endif %}
```

Under `whitespace = "suppress"`, `+` restores a span — `{#+ #}` is the idiom for keeping exactly one space between attributes.

Priority when markers collide: Suppress > Minimize > Preserve. Inline markers beat `#[template(whitespace = "suppress")]`, which beats `askama.toml` (`"preserve"` | `"suppress"` | `"minimize"`).

## Raw blocks

```jinja
{% raw %}{{ this is printed verbatim }}{% endraw %}
```

## HTML escaping

- Auto-escape is on for the HTML escaper's extensions: `html`, `htm`, `xml`, `j2`, `jinja`, `jinja2` (OWASP rules: `<`, `>`, `&`, `"`, `'`). `md`, `yml`, `txt`, `none` and the empty extension use the no-op text escaper.
- `{{ value | safe }}` — disable escaping for this value.
- `{{ value | escape }}` or `| e` — force escape in unescaped contexts.
- `#[template(escape = "none")]` — disable for the whole template.
- Implement `askama::filters::HtmlSafe` on a type to mark its `Display` output safe.

## Comments

```jinja
{# plain #}
{# outer {# nested #} still inside #}
```

## References, deref, `?`

```jinja
{% let x = &"value" %}
{% if *x == "value" %}…{% endif %}

{{ some_result? }}          {# unwrap or fail render #}
{% let v = other_result? %}
```

`?` works on `Result` only (not `Option`); on `Err` the render fails with `askama::Error::Custom` wrapping the error.

## Rust macros inside templates

```jinja
{% let text = format!("{}", 12) %}
```

Variables passed to Rust macros need explicit binding so Askama tracks them:

```jinja
{% let entity = entity %}
{{ test_macro!(entity) }}
```

## Rendering nested templates

```rust
#[derive(Template)]
#[template(source = "Section: {{ inner }}", ext = "txt")]
struct Outer { inner: Inner }
```

If `Inner` renders HTML, use `{{ inner | safe }}` or implement `HtmlSafe` on `Inner`.

For recursive structures, call `.render()` directly:

```jinja
{% for child in children %}{{ child.render()? }}{% endfor %}
```

## Working tips

- Templates are checked at compile time — a typo in a field name is a `cargo build` error, not a runtime one. When debugging, read the compiler error carefully; it points at the template path and line.
- Auto-escape interacts with `| safe`: never apply `| safe` to user-controlled data.
- `{% include %}` is compile-time; the path can't be dynamic — use `{% if %}`/`{% match %}` to choose between includes.
- Block inheritance flattens at compile time; `super()` calls are inlined.
- For exhaustive enum rendering, prefer `{% match %}` over `if let` chains so the compiler enforces coverage.
- An expression whose value is equivalent to `self` recurses forever — its `Display` impl re-evaluates the expression — and blows the stack at render time.
