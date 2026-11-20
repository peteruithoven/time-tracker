#!/usr/bin/env python3
"""A small mock of Fibery's commands API, for testing the app without a
real workspace. All data is made up and lives in memory.

    build-aux/fibery-mock.py [PORT] [--seed-logs N]      (port 8099 by default)

--seed-logs N starts with N made-up time logs of the demo user over the last
year (several a day), e.g. to measure syncing with a lot of data.

Connect the demo data to it with TIME_TRACKER_DEMO_FIBERY=http://127.0.0.1:PORT
(build-aux/ui-test.sh start --fibery does both). Every command is printed
to stdout, so you can check what the app sends.

It answers: the schema, workflow states, the current user, projects, tasks
(t1–t4), and time logs: created, updated and deleted logs are kept, so they
come back on the next pull. Unknown queries return no rows.

Scenarios change the data the way things can go wrong in a real workspace,
to try the app's sync pause:

    curl -d '{"scenario": "delete-logs", "count": 6}' 127.0.0.1:8099/mock

  delete-logs     delete the `count` newest logs
  reassign        give the `count` newest logs to another user
  zero-hours      set hours to 0 and clear start/end of `count` logs
  empty-hours     clear hours, start and end of `count` logs (a renamed field)
  drop-task-link  clear the task of `count` logs
  move-task       move task t1 (and its logs) to the other project
  flood           add `count` new logs
  empty           return no time logs at all (lost access)
  reset           undo all scenarios
"""

import copy
import itertools
import json
import random
import sys
from datetime import datetime, timedelta, timezone
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
EMPTY = False  # scenario "empty": no time logs come back
SAVED = None  # (LOGS, TASKS) before the first scenario


def now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.000Z")


def project_row(pid):
    return next(({"project_id": i, "project_name": n, "project_state": s, "project_final": f}
                 for i, n, s, f in PROJECTS if i == pid), {})


def seed_logs(count, days=365):
    """Made-up logs of the demo user: working hours on weekdays, newest last."""
    rng = random.Random(42)
    today = datetime.now(timezone.utc).replace(hour=0, minute=0, second=0, microsecond=0)
    for _ in range(count):
        day = today - timedelta(days=rng.randrange(1, days))
        start = day + timedelta(hours=8 + rng.randrange(0, 9), minutes=rng.choice([0, 15, 30, 45]))
        end = start + timedelta(minutes=rng.choice([30, 45, 60, 90, 120]))
        log_id = f"seed-{next(new_ids)}"
        LOGS[log_id] = {
            "Projects/Task": {"fibery/id": rng.choice(TASKS)[0]},
            "Projects/User": {"fibery/id": "me"},
            "Projects/Date": day.strftime("%Y-%m-%d"),
            "Projects/Start": start.strftime("%Y-%m-%dT%H:%M:%S.000Z"),
            "Projects/End": end.strftime("%Y-%m-%dT%H:%M:%S.000Z"),
            "Projects/Time spent": round((end - start).total_seconds() / 3600, 2),
            "Projects/Notes": "",
            "modified": now(),
        }


def newest_logs(count):
    ordered = sorted(LOGS.items(), key=lambda item: (item[1].get("Projects/Date") or "", item[1].get("Projects/Start") or ""))
    return [log for _, log in ordered[-count:]] if count else []


def scenario(name, count):
    global EMPTY, SAVED, TASKS
    if SAVED is None:
        SAVED = (copy.deepcopy(LOGS), list(TASKS))
    if name == "reset":
        LOGS.clear()
        LOGS.update(SAVED[0])
        TASKS = SAVED[1]
        EMPTY = False
        SAVED = None
        return "reset"
    if name == "empty":
        EMPTY = True
        return "time logs come back empty"
    if name == "delete-logs":
        for log in newest_logs(count):
            for key in [k for k, v in LOGS.items() if v is log]:
                del LOGS[key]
    elif name == "reassign":
        for log in newest_logs(count):
            log["Projects/User"] = {"fibery/id": "someone-else"}
    elif name in ("zero-hours", "empty-hours"):
        for log in newest_logs(count):
            log["Projects/Time spent"] = 0 if name == "zero-hours" else None
            log["Projects/Start"] = log["Projects/End"] = None
    elif name == "drop-task-link":
        for log in newest_logs(count):
            log["Projects/Task"] = None
    elif name == "move-task":
        TASKS = [(i, n, s, f, "p2" if i == "t1" else p) for i, n, s, f, p in TASKS]
    elif name == "flood":
        seed_logs(count, days=30)
    else:
        return f"unknown scenario {name}"
    for log in LOGS.values():
        log["modified"] = now()
    return f"{name} applied"


def query(q, params):
    source = q["q/from"]
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
        if EMPTY:
            return []
        tasks = {t[0]: t for t in TASKS}
        user = params.get("$user")
        since = params.get("$since") or ""
        rows = []
        for log_id, e in sorted(LOGS.items()):
            if user and (e.get("Projects/User") or {}).get("fibery/id", user) != user:
                continue
            if (e.get("Projects/Date") or "") < since:
                continue
            task_id = (e.get("Projects/Task") or {}).get("fibery/id")
            task = tasks.get(task_id)
            rows.append({
                "id": log_id, "modified": e["modified"], "date": e.get("Projects/Date"),
                "hours": e.get("Projects/Time spent"), "note": e.get("Projects/Notes"),
                "start": e.get("Projects/Start"), "end": e.get("Projects/End"),
                "task_id": task_id, "task_name": task[1] if task else None,
                **(project_row(task[4]) if task else {}),
            })
        offset = q.get("q/offset", 0)
        return rows[offset:offset + q.get("q/limit", len(rows))]
    if q.get("q/offset", 0):
        return []  # everything else fits on the first page
    return []


def run(command, args):
    if command == "fibery.schema/query":
        return SCHEMA
    if command == "fibery.entity/query":
        return query(args["query"], args.get("params") or {})
    entity = args.get("entity", {})
    if command == "fibery.entity/create":
        new_id = f"{args['type'].split('/')[-1].lower().replace(' ', '-')}-{next(new_ids)}"
        if args["type"] == "Projects/Time log":
            LOGS[new_id] = {**entity, "modified": now()}
        elif args["type"] == "Projects/Task":
            # Tasks created in the app come back in later pulls.
            project = (entity.get("Projects/Project") or {}).get("fibery/id")
            TASKS.append((new_id, entity.get("Projects/name", ""), "Open", False, project))
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
        if self.path == "/mock":
            message = scenario(body.get("scenario", ""), int(body.get("count", 0)))
            print("SCENARIO", message, flush=True)
            out = (message + "\n").encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(out)))
            self.end_headers()
            self.wfile.write(out)
            return
        print("CMD", body["command"], json.dumps(body.get("args", {}))[:300], flush=True)
        result = run(body["command"], body.get("args", {}))
        out = json.dumps({"success": True, "result": result}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)


if __name__ == "__main__":
    args = sys.argv[1:]
    if "--seed-logs" in args:
        i = args.index("--seed-logs")
        seed_logs(int(args[i + 1]))
        del args[i:i + 2]
    port = int(args[0]) if args else 8099
    print(f"Fibery mock on http://127.0.0.1:{port}", flush=True)
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
