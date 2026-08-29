"""`python -m powerio.mcp` serves the tool surface over stdio.

Guarded so a module walk (stubtest, an import-everything linter) can import
this file without starting the stdio server; runpy still executes it as
__main__. The attribute import routes through the package's __getattr__,
so a Python older than 3.10 gets the version requirement instead of a
TypeAlias import failure from the server module.
"""

if __name__ == "__main__":
    from . import main

    main()
