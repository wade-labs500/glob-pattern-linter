# globlint

A validating parser and pretty printer for shell-style file glob patterns
(the `*`, `**`, `?`, `[...]`, `{...,...}` kind you write in `.gitignore`
files, build tool configs, and CLI `--include` flags).

## The problem

Glob syntax looks trivial until you have to write a lot of it by hand.
Patterns fail silently in ways that are easy to miss on review:

- `src/**foo` - `**` is supposed to match across directories, but mixing it
  with other characters in the same path component makes most glob engines
  treat it as a plain `*` instead, which is rarely what was intended.
- `[a-z` - a forgotten closing bracket. Some implementations treat the rest
  of the string as literal text instead of erroring, so the pattern quietly
  matches nothing.
- `[z-a]` - a backwards character range, again often accepted and just
  never matches.
- `a//b` - an empty path component from a stray double slash.

None of these raise an error in most shells or glob crates; they just
produce a pattern that doesn't do what the author thinks it does. This
tool parses the pattern into a real AST, rejects the cases above with a
column number, and prints the pattern back out in a canonical form so two
patterns that mean the same thing look the same.

## Usage

As a CLI:

```
$ cargo run -- 'src/**/*.rs' '{b,a,a}.txt' 'a**b' '[z-a]'
src/**/*.rs
{b,a,a}.txt => {a,b}.txt
a**b: invalid: column 1: '**' must occupy a whole path component
[z-a]: invalid: column 1: invalid range 'z-a': start is greater than end
```

Note the second pattern: `{b,a,a}` and `{a,b}` match exactly the same set
of strings, so the canonical form sorts alternation branches and removes
duplicates. `a**b/**/**c` would similarly collapse redundant `**/**`
chains.

As a library:

```rust
use globlint::{parse, pretty_print};

fn main() {
    match parse("src/**/*.{rs,toml}") {
        Ok(glob) => println!("parsed {} segments", glob.segments().len()),
        Err(e) => eprintln!("bad pattern: {e}"),
    }

    // pretty_print is parse() + rendering the AST back to a canonical string
    assert_eq!(pretty_print("{b,a}.txt").unwrap(), "{a,b}.txt");
}
```

## Supported syntax

- literals, escaped with `\` when they'd otherwise be special
- `*` - any run of characters within one path component
- `**` - any number of path components; must be the entire component
  (`a/**/b`, not `a/x**/b`)
- `?` - exactly one character
- `[abc]`, `[a-z]`, `[!abc]` - character classes, with `!` or `^` for
  negation and the classic "a leading `]` is a literal" rule
- `{a,b,c}` - alternation, branches can themselves contain any of the above
- `/` - path separator

Not supported: POSIX bracket expressions like `[[:alpha:]]`, extended glob
operators (`@(...)`, `+(...)`), and brace *expansion* semantics beyond
simple alternation (no `{1..5}` ranges).

## Status

First pass. The parser, validator, and pretty printer are complete for the
grammar above; there's no matcher yet (turning a `Glob` into "does this
path match" is separate future work, see below).
