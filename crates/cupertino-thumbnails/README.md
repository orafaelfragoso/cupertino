# cupertino-thumbnails

The freedesktop.org thumbnail cache (`$XDG_CACHE_HOME/thumbnails/<normal|large|x-large|xx-large>/<md5 of the file URI>.png`)
for every application that shows pictures: `Thumbnailer::lookup` returns a valid cached thumbnail, `create` decodes the
image (PNG, JPEG, WebP), scales it to the cache size and writes it with the standard text chunks, and
`get_or_create` does both; `pixels` returns the cached or created thumbnail decoded to straight RGBA, shrunk to fit an
edge, and `root` is the cache directory, whose own files are never thumbnailed. A failed image is remembered under `fail/<application>/` so it is not tried again. The MIME type
written into a thumbnail is the one of the format found in the file's content. The file is opened without blocking and
must be a regular file of at most 256 MiB, and the picture at most 8192 pixels a side: a pipe named `x.png`, a huge file or
a decompression bomb fails at once and is remembered as a failure.

Used by `cupertino-files` and `cupertino-settings`, so both share one cache.
