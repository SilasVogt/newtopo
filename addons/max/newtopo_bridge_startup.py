# 3ds Max startup script for the newtopo bridge. Copy into the user
# startup scripts folder; it adds the add-on folder to sys.path and starts
# listening once the UI is up. UNTESTED (no 3ds Max available).

import os
import sys

ADDON_DIR = os.environ.get("NEWTOPO_BRIDGE_DIR", r"C:\path\to\newtopo\addons\max")

if ADDON_DIR not in sys.path:
    sys.path.insert(0, ADDON_DIR)

try:
    import newtopo_bridge

    newtopo_bridge.start()
except Exception as exc:  # never break Max startup
    print("newtopo bridge failed to start:", exc)
