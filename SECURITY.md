# Security

Report vulnerabilities through GitHub's private vulnerability reporting on this
repository (Security tab → Report a vulnerability). Do not open a public issue
for anything exploitable.

In scope: memory safety defects reachable through the C ABI or the Python
bindings, and parser behavior on untrusted case files (the parsers are written
to reject malformed input with an error, never to crash or corrupt memory).

Only the latest release is supported.

## Parsing an untrusted case

OpenDSS `Redirect`, `Compile`, and `Buscoords` includes resolve inside the case file's own directory subtree: an include that climbs out with `..`, names an absolute path outside it, or escapes through a symbolic link is refused. Inside the subtree nothing is restricted. A case file reads any file placed beneath its directory, and the leading token of each line the parser does not recognize comes back in the parser warnings, so give an untrusted case a directory of its own. Parsing from a string follows no includes at all.
