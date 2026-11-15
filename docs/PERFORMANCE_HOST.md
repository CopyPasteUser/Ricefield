# RiceField 1.1: memory profiling on Linux Mint

The CI Xvfb measurement is an isolated software-GL baseline, not a
measurement of the owner's ThinkPad or a controlled Thunar comparison.
This read-only script samples an **already running** same-user process by PID.
It does not launch, terminate, reconfigure or modify the application.

Run RiceField and Thunar on the **same system**, with the **same directory**,
view, image thumbnails and number of tabs. Let each settle before sampling;
measure both idle and while repeatedly scrolling the same image-heavy folder.

```bash
ricefield_pid=$(pgrep -n -u "$(id -u)" -x ricefield)
bash scripts/profile_host_memory.sh "$ricefield_pid" 20

thunar_pid=$(pgrep -n -u "$(id -u)" -x thunar)
bash scripts/profile_host_memory.sh "$thunar_pid" 20
```

Check that the PIDs refer to the intended application processes. If a process
is not already running, start it normally and repeat. The script refuses
invalid/inaccessible PIDs and does not read credentials or file contents.

Compare **PSS** for a better estimate of memory attributable to each process
(shared libraries are proportionally accounted for), RSS and peak variation,
thread counts and CPU percentage of a single core. For reproducibility,
record the exact RiceField commit, Thunar version, Mint session (X11/Wayland),
GPU, RAM, directory file count and test conditions. A single idle measurement
does not prove improvement under scrolling, cold-start or low-RAM pressure.

The lazy thumbnail decoder in PR #186 changes thread creation during image
browsing, not necessarily idle memory: the decoder is absent before the first
request and exits after inactivity. Compare multiple runs; do not interpret
one Xvfb sample as a hardware gain.
