# patches/

Source patches applied to the upstream desktop client before the build. Empty on purpose: the branded
build changes only CMake variables (`OEM.cmake`) and theme files.

A patch goes here only when a change to upstream code is unavoidable. Each patch must be a `git
format-patch` file against the exact upstream tag in `oem.json`, numbered (`0001-...patch`), with a
one-line reason at the top. Every patch is part of the corresponding source (GPL) and must be listed in
`README.md`.
