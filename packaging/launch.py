"""PyInstaller entry script: opendrs/__main__.py uses relative imports, so import it as a module."""

from opendrs.__main__ import main

raise SystemExit(main())
