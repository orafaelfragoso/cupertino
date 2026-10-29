# cupertino-key-bindings

Key chords and the text-editing binding table: `Chord`/`Key`/`NamedKey` with logical modifiers, the standard
`BindingTable` with `TextAction`s, override text and the `Resolver` for multi-chord sequences. MPL-2.0.

It reads no settings and watches no files, so CupertinoKit's text fields can use it. `cupertino-keyboard` re-exports
it and adds `binding::load_default`, which reads `~/.config/cupertino/keybindings.conf`.

```sh
cargo test -p cupertino-key-bindings
```
