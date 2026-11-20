#!/usr/bin/env bash
# Lists the tests that failed and then passed on a retry, from the JUnit report nextest wrote
# (`.config/nextest.toml`, profile `ci`), as a Markdown section for a job summary:
#   .github/scripts/flaky-report.sh <junit.xml>...
# Nothing is printed when no test was flaky. A test that failed all its attempts is not here: it
# fails its job. In the report a flaky test has one <flakyFailure> per failed attempt, and the
# attempt that passed is the last one.
set -euo pipefail

if [ $# -eq 0 ]; then
  echo "flaky-report.sh: a JUnit file is needed" >&2
  exit 2
fi

python3 -I - "$@" <<'PY'
import sys
import xml.etree.ElementTree as ET

flaky = []
for path in sys.argv[1:]:
    for case in ET.parse(path).getroot().iter("testcase"):
        failed = len(case.findall("flakyFailure")) + len(case.findall("flakyError"))
        if failed and case.find("failure") is None and case.find("error") is None:
            name = f'{case.get("classname", "")} {case.get("name", "")}'.strip()
            flaky.append((name, failed + 1))

if flaky:
    print("## Flaky tests")
    print()
    print("These passed on a retry: fix them, a retry hides nothing from here.")
    print()
    print("| Test | Attempts |")
    print("|---|---|")
    for name, attempts in sorted(flaky):
        print(f"| `{name}` | {attempts} |")
PY
