# Serial console lab checklist (manual)

Run this on a lab PC with WSL2, usbipd-win, and a USB-UART adapter. The box
gate does not cover real hardware. Record results in the PR that adds or
changes native serial support. Do not paste board console logs that contain
secrets. `dmuxai term` is Linux/WSL only: there is no Windows ConPTY build, and
there is no native `ssh` session kind (run `ssh` inside a `shell` session).

dmuxai never runs `usbipd` from WSL. Attach the adapter yourself from an
elevated Windows PowerShell before opening a session, or expect a
`needs_usbipd_attach` error if the node is missing.

1. Not-attached case (device not visible in WSL):
   `dmuxai term open board --serial /dev/ttyUSB0 --baud 115200 --usbipd-busid <id> --json`
   must exit 3 with `error_kind` `needs_usbipd_attach`. JSON includes `busid` and
   the commands `usbipd bind --busid <id>` (first time only) and
   `usbipd attach --wsl --busid <id>`. Human-readable output prints the same
   guidance. Repeat without `--usbipd-busid`: still `needs_usbipd_attach`, with a
   `<busid>` placeholder plus `usbipd list`. dmuxai must not invoke `usbipd.exe`.
2. In an elevated Windows PowerShell run `usbipd list`, then
   `usbipd bind --busid <id>` (first time only) and
   `usbipd attach --wsl --busid <id>`. After the node appears in WSL:
   `dmuxai term open board --serial /dev/ttyUSB0 --baud 115200 --usbipd-busid <id> --wait-for 'U-Boot' --timeout 60s`
   (or the banner the board actually prints). Open should succeed; attach is not
   retried from WSL.
3. After a shell prompt appears, `dmuxai term exec board 'uname -a' --timeout 15s --json`
   and confirm `exit_code` and cleaned `text`.
4. Power-cycle the board, `dmuxai term wait board --match 'Hit any key' --timeout 60s`,
   then `dmuxai term send board --keys Space`.
5. Unplug the adapter: `dmuxai term show board --json` reports `state: "lost"`.
   Replug, re-attach from Windows PowerShell if needed, then
   `dmuxai term reopen board --json`.
6. `dmuxai term send board reboot --json` is blocked (exit 7, `approval_id`).
   `dmuxai term approve <id>` lets that one input through.
7. Optional: `--kind cmd -- picocom -b 115200 /dev/ttyUSB0` send/read still works.

On WSL1, `--serial COM3` must open `/dev/ttyS3`. A permission error must mention
the dialout group and `wsl --shutdown`, never `chmod 666`.
