# cupertino-ui-render

CupertinoKit rendering: the `DisplayList` core paints each window into, `Damage`, and the `UiRenderer` trait that
draws a list into premultiplied ARGB8888 pixels. MPL-2.0. Today the crate declares the public types; the CPU backend follows.
