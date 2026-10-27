# cupertino-thumbnails

The freedesktop.org thumbnail cache (`$XDG_CACHE_HOME/thumbnails/<normal|large|x-large|xx-large>/<md5 of the file URI>.png`)
for every application that shows pictures: `Thumbnailer::lookup` returns a valid cached thumbnail, `create` decodes the
image (PNG, JPEG, GIF, WebP, BMP), scales it to the cache size and writes it with the standard text chunks, and
`get_or_create` does both. A failed image is remembered under `fail/<application>/` so it is not tried again. The MIME type
written into a thumbnail comes from the file name.

Used by `cupertino-files` and `cupertino-settings`, so both share one cache.
