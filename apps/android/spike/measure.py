#!/usr/bin/env python3
"""The real-phone run of the push spike (#823): ten tries, the table for the pull request.

Run beside `serve.sh`, the phone on USB with `adb` (the USB cable carries only adb: the push and
the answer go over the network named by --network).

    apps/android/spike/measure.py --gate http://<address the phone reaches>:7470 --network wifi

Each try: the app killed, the phone forced into Doze and its screen off, a request of the fake
agent held, its push sent; you wake the screen and tap Allow; the script waits for the phone's
report and writes the row. The table goes to stdout and to measure-<network>.md.
"""

import argparse
import json
import subprocess
import sys
import time
import urllib.request

APP = "org.flottille.spike"
LOCAL = "http://127.0.0.1:7470"


def adb(*args):
    return subprocess.run(["adb", *args], capture_output=True, text=True, check=False).stdout.strip()


def shell(command):
    return adb("shell", command)


def gate(method, path):
    request = urllib.request.Request(LOCAL + path, method=method, data=b"{}" if method == "POST" else None)
    with urllib.request.urlopen(request, timeout=60) as answer:
        return json.load(answer)


def events():
    return gate("GET", "/state")["events"]


def now_ms():
    return int(time.time() * 1000)


def offset_ms():
    """The phone's clock minus this machine's, read over adb, the best of five."""
    best = None
    for _ in range(5):
        before = now_ms()
        phone = int(shell("date +%s%N")) // 1_000_000
        after = now_ms()
        if best is None or after - before < best[0]:
            best = (after - before, phone - (before + after) // 2)
    return best[1]


def join(public):
    adb("reverse", "tcp:7448", "tcp:7448")
    code = gate("GET", "/invitation")["code"]
    shell(f"pm grant {APP} android.permission.POST_NOTIFICATIONS")
    shell(f"am start -W -n {APP}/.MainActivity --es code {code} --es gate {public}")
    print("Joining: if the phone asks which distributor, choose ntfy.", file=sys.stderr)
    deadline = time.time() + 120
    while not gate("GET", "/state")["registered"]:
        if time.time() > deadline:
            sys.exit("the phone did not register its endpoint in 2 minutes: see the app's screen")
        time.sleep(1)
    endpoint = [e for e in events() if "registered" in e][-1]["endpoint"]
    print(f"Registered: the runner pushes to {endpoint.split('/')[2]}", file=sys.stderr)


def one_try(index, offset):
    shell("input keyevent KEYCODE_HOME")
    time.sleep(1)
    shell(f"am kill {APP}")
    shell("dumpsys battery unplug")
    shell("dumpsys deviceidle force-idle")
    shell("input keyevent KEYCODE_SLEEP")
    time.sleep(2)
    killed = shell(f"pidof {APP}") == ""
    idle = shell("dumpsys deviceidle get deep")
    held = gate("POST", "/hold")
    number, pushed = held["number"], held["pushed"]
    print(f"Try {index}: request #{number} pushed. Wake the screen and tap Allow.", file=sys.stderr)
    deadline = time.time() + 180
    while True:
        reports = [e["report"] for e in events() if "report" in e and e["report"].get("number") == number]
        if reports:
            report = reports[-1]
            break
        if time.time() > deadline:
            return {"try": index, "number": number, "killed": killed, "idle": idle, "failed": "no report in 3 minutes"}
        time.sleep(0.5)
    calls = [e for e in events() if e.get("call") == "approval.approve" and (e.get("params") or {}).get("number") == number]
    return {
        "try": index,
        "number": number,
        "killed": killed,
        "idle": idle,
        "push_to_notification_ms": report["notified"] - offset - pushed if report.get("notified") else None,
        "tap_to_answer_ms": report.get("answerMs"),
        "tap_to_gate_ms": calls[-1]["received"] - (report["tapped"] - offset) if calls else None,
        "read_in_doze": report.get("read"),
        "read_ms": report.get("readMs"),
        "idle_at_push": report.get("idleAtPush"),
        "idle_at_tap": report.get("idleAtTap"),
        "answered": report.get("answered"),
        "error": report.get("error") or report.get("readError") or "",
    }


def table(network, rows):
    lines = [
        f"### {network}",
        "",
        "| try | killed | Doze | push → notification (ms) | tap → answer (ms) | read in Doze | answered | note |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for row in rows:
        read = row.get("read_in_doze")
        read = "not tried" if read is None else (f"yes, {row.get('read_ms')} ms" if read else "refused")
        lines.append(
            f"| {row['try']} | {'yes' if row['killed'] else 'no'} | {row['idle']} | "
            f"{row.get('push_to_notification_ms', '')} | {row.get('tap_to_answer_ms', '')} | {read} | "
            f"{'yes' if row.get('answered') else 'no'} | {row.get('failed') or row.get('error', '')} |"
        )
    pushes = sorted(r["push_to_notification_ms"] for r in rows if r.get("push_to_notification_ms") is not None)
    taps = sorted(r["tap_to_answer_ms"] for r in rows if r.get("answered") and r.get("tap_to_answer_ms") is not None)
    if pushes and taps:
        lines += ["", f"Median: push → notification {pushes[len(pushes) // 2]} ms, tap → answer "
                  f"{taps[len(taps) // 2]} ms; {len(taps)} of {len(rows)} answered."]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--gate", required=True, help="the gate's address as the phone reaches it")
    parser.add_argument("--network", required=True, choices=["wifi", "4g"])
    parser.add_argument("--tries", type=int, default=10)
    parser.add_argument("--join", action="store_true", help="join again (a new server was started)")
    options = parser.parse_args()
    if not adb("get-state") == "device":
        sys.exit("no phone on adb: plug it in and allow USB debugging")
    if options.join or not gate("GET", "/state")["registered"]:
        join(options.gate.rstrip("/"))
    offset = offset_ms()
    print(f"The phone's clock is {offset} ms ahead of this machine's.", file=sys.stderr)
    rows = []
    try:
        for index in range(1, options.tries + 1):
            rows.append(one_try(index, offset))
            print(json.dumps(rows[-1]), file=sys.stderr)
    finally:
        shell("dumpsys deviceidle unforce")
        shell("dumpsys battery reset")
    text = table(options.network, rows)
    with open(f"measure-{options.network}.md", "w") as out:
        out.write(text)
    print(text)


if __name__ == "__main__":
    main()
