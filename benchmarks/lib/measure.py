# Runs a command and prints its wall time (ms) and peak resident set size (KiB) as JSON on
# stderr's last line; the command's own output is discarded. Usage: python3 measure.py cmd…
import json, os, subprocess, sys, time

start = time.perf_counter()
child = subprocess.Popen(sys.argv[1:], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
_, status, usage = os.wait4(child.pid, 0)
elapsed = (time.perf_counter() - start) * 1000
stderr = child.stderr.read().decode(errors="replace")
code = os.waitstatus_to_exitcode(status)
sys.stderr.write(stderr)
sys.stderr.write("\n" + json.dumps({"ms": elapsed, "rss": usage.ru_maxrss, "status": code}) + "\n")
sys.exit(0)
