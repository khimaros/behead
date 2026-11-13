"""a process's output, line by line, for tests that wait on what it prints"""

import re
import threading

TIMEOUT = 10


class Lines:
    """collects a pipe's lines on a thread, so tests can wait for one"""

    def __init__(self, pipe):
        self.lines, self.changed = [], threading.Condition()
        threading.Thread(target=self.pump, args=(pipe,), daemon=True).start()

    def pump(self, pipe):
        for line in pipe:
            with self.changed:
                self.lines.append(line.decode(errors="replace").rstrip("\n"))
                self.changed.notify_all()

    def wait(self, pattern, timeout=TIMEOUT, count=1):
        """every line so far matching pattern, once there are count of them"""
        def matching():
            found = [line for line in self.lines if re.search(pattern, line)]
            return found if len(found) >= count else None
        with self.changed:
            found = self.changed.wait_for(matching, timeout)
        assert found, f"never saw {pattern!r} in:\n" + "\n".join(self.lines)
        return found
