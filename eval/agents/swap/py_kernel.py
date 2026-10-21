"""A minimal stateful Python kernel for the pandas arm (runs inside the sandbox).

    python -I py_kernel.py SECONDS < requests

Each request is one JSON line {"code": ...}; the code runs in one persistent namespace (variables
survive between calls, as in a notebook), the value of a final expression is printed, and the reply
is one JSON line {"out": stdout+stderr, "error": bool} on the original stdout. A call longer than
SECONDS is interrupted.
"""

import ast
import contextlib
import io
import json
import signal
import sys
import traceback


class Timeout(BaseException):
    pass


def alarm(*_):
    raise Timeout()


def main() -> None:
    limit = int(sys.argv[1])
    proto = sys.stdout
    ns = {"__name__": "__main__"}
    signal.signal(signal.SIGALRM, alarm)
    for line in sys.stdin:
        code = json.loads(line)["code"]
        buf, err = io.StringIO(), False
        with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(buf):
            signal.alarm(limit)
            try:
                tree = ast.parse(code)
                last = ast.Expression(tree.body.pop().value) if tree.body and isinstance(tree.body[-1], ast.Expr) else None
                exec(compile(tree, "<cell>", "exec"), ns)
                if last is not None:
                    value = eval(compile(last, "<cell>", "eval"), ns)
                    if value is not None:
                        print(repr(value))
            except Timeout:
                err = True
                print(f"[interrupted: the call took longer than {limit} s]")
            except BaseException:
                err = True
                traceback.print_exc()
            finally:
                signal.alarm(0)
        proto.write(json.dumps({"out": buf.getvalue(), "error": err}) + "\n")
        proto.flush()


if __name__ == "__main__":
    main()
