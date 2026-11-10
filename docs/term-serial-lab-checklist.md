# Serial console lab checklist (manual)

Run this on a lab PC with WSL2, usbipd-win, and a USB-UART adapter. The box
gate does not cover real hardware. Record results in the PR that adds native
serial support (PR1b). Do not paste board console logs that contain secrets.

1. Attach the adapter: `usbipd.exe list`, then
   `dmuxai term open board --serial /dev/ttyUSB0 --baud 115200 --wait-for 'U-Boot' --timeout 60s`
   (or the banner the board actually prints). If the node is missing, dmuxai
   should invoke `usbipd.exe attach --wsl --busid <id>` when a busid is given.
2. After a shell prompt appears, `dmuxai term exec board 'uname -a' --timeout 15s --json`
   and confirm `exit_code` and cleaned `text`.
3. Power-cycle the board, `dmuxai term wait board --match 'Hit any key' --timeout 60s`,
   then `dmuxai term send board --keys Space`.
4. Unplug the adapter: `dmuxai term show board --json` reports `state: "lost"`.
   Replug, re-attach if needed, then `dmuxai term reopen board --json`.
5. `dmuxai term send board reboot --json` is blocked (exit 7, `approval_id`).
   `dmuxai term approve <id>` lets that one input through.
6. Optional: `--kind cmd -- picocom -b 115200 /dev/ttyUSB0` send/read still works.

On WSL1, `--serial COM3` must open `/dev/ttyS3`. A permission error must mention
the dialout group and `wsl --shutdown`, never `chmod 666`.
