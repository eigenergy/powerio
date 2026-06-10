# Security

Report vulnerabilities through GitHub's private vulnerability reporting on this
repository (Security tab → Report a vulnerability). Do not open a public issue
for anything exploitable.

In scope: memory safety defects reachable through the C ABI or the Python
bindings, and parser behavior on untrusted case files (the parsers are written
to reject malformed input with an error, never to crash or corrupt memory).

Only the latest release is supported.
