#!/usr/bin/env python3
"""A small mock of Fibery's commands API, for testing the app without a
real workspace. All data is made up and lives in memory.

    build-aux/fibery-mock.py [PORT]      (default 8099)

Connect the demo data to it with TIME_TRACKER_DEMO_FIBERY=http://127.0.0.1:PORT
(build-aux/ui-test.sh start --fibery does both). Every command is printed
to stdout, so you can check what the app sends.

It answers: the schema, workflow states, the current user, projects, tasks
(t1–t4), and time logs: created, updated and deleted logs are kept, so they
come back on the next pull. Unknown queries return no rows.
"""

import itertools
import json
import sys
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, HTTPServer


def field(name, ty, **meta):
    return {"fibery/name": name, "fibery/type": ty, "fibery/meta": meta}


SCHEMA = {"fibery/types": [
    {"fibery/name": "fibery/user", "fibery/meta": {"fibery/domain?": False},
     "fibery/fields": [field("user/name", "fibery/text")]},
    {"fibery/name": "Projects/Time log", "fibery/meta": {"fibery/domain?": True}, "fibery/fields": [
        field("fibery/id", "fibery/uuid", **{"fibery/readonly?": True}),
        field("Projects/End", "fibery/date-time"), field("Projects/Start", "fibery/date-time"),
        field("Projects/Task", "Projects/Task"), field("Projects/User", "fibery/user"),
        field("Projects/Time spent", "fibery/decimal"), field("Projects/Billable hours", "fibery/decimal"),
        field("Projects/Date", "fibery/date"), field("Projects/Notes", "fibery/text")]},
    {"fibery/name": "Projects/Task", "fibery/meta": {"fibery/domain?": True}, "fibery/fields": [
        field("Projects/Timing", "fibery/bool"), field("Projects/name", "fibery/text", **{"ui/title?": True}),
        field("Projects/Project", "Projects/Project"), field("Projects/Epic", "Projects/Epic"),
        field("assignments/assignees", "fibery/user", **{"fibery/collection?": True}),
        field("Projects/Time logs", "Projects/Time log", **{"fibery/collection?": True}),
        field("workflow/state", "workflow/state_Projects/Task")]},
    {"fibery/name": "Projects/Project", "fibery/meta": {"fibery/domain?": True}, "fibery/fields": [
        field("Projects/name", "fibery/text", **{"ui/title?": True}),
        field("workflow/state", "workflow/state_Projects/Project")]},
    {"fibery/name": "Projects/Epic", "fibery/meta": {"fibery/domain?": True},
     "fibery/fields": [field("Projects/name", "fibery/text", **{"ui/title?": True})]},
    # A second database that could hold time logs, to try switching.
    {"fibery/name": "Ops/Work entry", "fibery/meta": {"fibery/domain?": True}, "fibery/fields": [
        field("Ops/Day", "fibery/date"), field("Ops/Hours", "fibery/int"), field("Ops/Ticket", "Ops/Ticket")]},
    {"fibery/name": "Ops/Ticket", "fibery/meta": {"fibery/domain?": True},
     "fibery/fields": [field("Ops/title", "fibery/text", **{"ui/title?": True})]},
]}

STATES = {
    "workflow/state_Projects/Task": [("Open", False), ("Waiting", False), ("Doing", False), ("Done", True)],
    "workflow/state_Projects/Project": [("Active", False), ("On hold", False), ("Billing", True), ("Done", True)],
}
# id, name, state, finished
PROJECTS = [("p1", "Website relaunch", "Active", False), ("p2", "Mobile app", "Billing", True)]
# id, name, state, finished, project id
TASKS = [
    ("t1", "Wireframes", "Open", False, "p1"),
    ("t2", "Design system", "Waiting", False, "p1"),
    ("t3", "Payment flow", "Doing", False, "p2"),
    ("t4", "Briefing", "Done", True, "p1"),
]
LOGS = {}  # id -> entity, as the app sent it
new_ids = itertools.count(1)


def now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.000Z")


def project_row(pid):
    return next(({"project_id": i, "project_name": n, "project_state": s, "project_final": f}
                 for i, n, s, f in PROJECTS if i == pid), {})


def query(q):
    source = q["q/from"]
    if q.get("q/offset", 0):
        return []  # everything fits on the first page
    if source in STATES:
        return [{"name": n, "final": f} for n, f in STATES[source]]
    if source == "fibery/user":
        return [{"id": "me", "name": "Demo User"}]
    if source == "Projects/Project":
        return [{"id": i, "name": n, "state": s, "final": f} for i, n, s, f in PROJECTS]
    if source == "Projects/Task":
        return [{"id": i, "name": n, "state": s, "final": f, "modified": now(), **project_row(p)}
                for i, n, s, f, p in TASKS]
    if source == "Projects/Time log":
        tasks = {t[0]: t for t in TASKS}
        rows = []
        for log_id, e in LOGS.items():
            task_id = (e.get("Projects/Task") or {}).get("fibery/id", "")
            task = tasks.get(task_id)
            rows.append({
                "id": log_id, "modified": e["modified"], "date": e.get("Projects/Date"),
                "hours": e.get("Projects/Time spent", 0), "note": e.get("Projects/Notes"),
                "start": e.get("Projects/Start"), "end": e.get("Projects/End"),
                "task_id": task_id, "task_name": task[1] if task else "Unknown task",
                **(project_row(task[4]) if task else {}),
            })
        return rows
    return []


def run(command, args):
    if command == "fibery.schema/query":
        return SCHEMA
    if command == "fibery.entity/query":
        return query(args["query"])
    entity = args.get("entity", {})
    if command == "fibery.entity/create":
        new_id = f"{args['type'].split('/')[-1].lower().replace(' ', '-')}-{next(new_ids)}"
        if args["type"] == "Projects/Time log":
            LOGS[new_id] = {**entity, "modified": now()}
        return {"fibery/id": new_id}
    if command == "fibery.entity/update":
        log = LOGS.get(entity.get("fibery/id"))
        if log is not None:
            log.update(entity)
            log["modified"] = now()
        return entity
    if command == "fibery.entity/delete":
        LOGS.pop(entity.get("fibery/id"), None)
        return None
    return None


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        print("CMD", body["command"], json.dumps(body.get("args", {})), flush=True)
        result = run(body["command"], body.get("args", {}))
        out = json.dumps({"success": True, "result": result}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8099
    print(f"Fibery mock on http://127.0.0.1:{port}", flush=True)
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
