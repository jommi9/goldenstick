# macOS hub regression fixture

`macos-kingston-via-hub.json` is reduced from a diagnostics report collected on 3 October 2026 with a Kingston DataTraveler 3.0 connected through a VIA Labs USB 3.0 hub. Before the fix, enumeration assigned the hub vendor ID `0x2109` to the disk instead of Kingston ID `0x0951`.

The fixture preserves the storage branch topology, USB descriptor fields and disk geometry required for replay. Internal disks and unrelated registry branches were removed. Serial numbers and the volume label were replaced, UUIDs were removed, and the timestamp was reset. The original report remains local and is excluded from version control.

The regression was run before the parser change and failed on the vendor ID assertion. It tests the captured connection path; it does not establish compatibility with other hubs or DJ players.
