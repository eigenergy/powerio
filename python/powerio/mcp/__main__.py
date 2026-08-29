"""`python -m powerio.mcp` serves the tool surface over stdio.

The attribute import routes through the package's __getattr__, so a
Python older than 3.10 gets the version requirement instead of a
TypeAlias import failure from the server module.
"""

if __name__ == "__main__":
    from . import main

    main()
