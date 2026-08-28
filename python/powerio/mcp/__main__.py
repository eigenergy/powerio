from .server import main

# Guarded so a module walk (stubtest, an import-everything linter) can import
# this file without starting the stdio server; `python -m powerio.mcp` still
# runs it, since runpy executes it as __main__.
if __name__ == "__main__":
    main()
