/* The mock editor's DOM. The user thinks in operations: every declared @http
   operation gets a card (mock it, edit the mocked response, unmock it), with
   the transport route shown only as fine print. The response body starts as a
   valid sample built from the operation's declared output type. Custom routes
   remain available for anything outside the declared surface. Storage stays
   the mocks.json routes table (share links and runs unchanged); a route row
   whose path is the operation's template is that operation's mock. */
import {
  emptyForm,
  formFromJson,
  formToJson,
  validate,
  METHODS,
  type MockForm,
  type RouteRow,
} from "./mockform";
import { opCatalog, type OpInfo } from "./mocksample";

export interface MockUi {
  json(): string;
  setJson(text: string): void;
  setIr(ir: string | null): void;
}

export function createMockUi(options: {
  root: HTMLElement;
  rawEditor: { get(): string; set(text: string): void; container: HTMLElement };
  onChange: () => void;
}): MockUi {
  const $ = <T extends HTMLElement>(sel: string): T => {
    const el = options.root.querySelector<T>(sel);
    if (!el) throw new Error(`missing ${sel}`);
    return el;
  };
  const formEl = $("#mocks-form");
  const modeBtn = $("#mocks-mode");
  const envRows = $("#env-rows");
  const opRows = $("#op-rows");
  const routeRows = $("#route-rows");
  const passthrough = $<HTMLInputElement>("#mocks-passthrough");

  let form: MockForm = emptyForm();
  let rawMode = false;
  let catalog: OpInfo[] = [];

  const rowOf = (op: OpInfo): RouteRow | undefined =>
    op.route
      ? form.routes.find((r) => r.method === op.route!.method && r.path === op.route!.path)
      : undefined;

  function issueNotes(card: HTMLElement, rowIndex: number): void {
    for (const issue of validate(form).filter((x) => x.index === rowIndex)) {
      const el = card.querySelector<HTMLElement>(
        issue.field === "path"
          ? ".route-path"
          : issue.field === "status"
            ? ".route-status"
            : ".route-body",
      );
      el?.classList.add("field-error");
      const note = document.createElement("div");
      note.className = "route-issue";
      note.textContent = issue.message;
      card.append(note);
    }
  }

  function statusInput(row: RouteRow): HTMLInputElement {
    const status = document.createElement("input");
    status.className = "route-status";
    status.value = row.status;
    status.title = "HTTP status";
    status.addEventListener("input", () => {
      row.status = status.value;
      changed(false);
    });
    return status;
  }

  function bodyArea(row: RouteRow): HTMLTextAreaElement {
    const body = document.createElement("textarea");
    body.className = "route-body";
    body.rows = 4;
    body.spellcheck = false;
    body.value = row.body;
    body.title = "Response body (JSON)";
    body.addEventListener("input", () => {
      row.body = body.value;
      changed(false);
    });
    return body;
  }

  function render(): void {
    envRows.replaceChildren();
    form.env.forEach((row, i) => {
      const line = document.createElement("div");
      line.className = "env-row";
      const key = document.createElement("input");
      key.className = "env-key";
      key.placeholder = "NAME";
      key.value = row.key;
      key.addEventListener("input", () => {
        row.key = key.value;
        changed(false);
      });
      const value = document.createElement("input");
      value.className = "env-value";
      value.placeholder = "value ($MOCK = mock server)";
      value.value = row.value;
      value.addEventListener("input", () => {
        row.value = value.value;
        changed(false);
      });
      const rm = document.createElement("button");
      rm.className = "row-remove";
      rm.textContent = "×";
      rm.title = "Remove";
      rm.addEventListener("click", () => {
        form.env.splice(i, 1);
        changed(true);
      });
      line.append(key, value, rm);
      envRows.append(line);
    });

    /* One card per declared operation. */
    opRows.replaceChildren();
    for (const op of catalog) {
      const card = document.createElement("div");
      card.className = "op-card";
      const head = document.createElement("div");
      head.className = "op-head";
      const name = document.createElement("span");
      name.className = "op-name";
      name.textContent = op.name;
      const route = document.createElement("span");
      route.className = "op-route";
      route.textContent = op.route
        ? `${op.route.method} ${op.route.path}`
        : `ext impl${op.implLangs.length > 0 ? `: ${op.implLangs.join(", ")}` : ""}`;
      head.append(name, route);
      if (!op.route) {
        /* A bespoke operation runs its own code; there is no transport to
           answer for it, so the card is informational. */
        card.append(head);
        opRows.append(card);
        continue;
      }
      const row = rowOf(op);
      if (!row) {
        const mock = document.createElement("button");
        mock.className = "mini-btn";
        mock.textContent = "mock";
        mock.title = "Answer this operation with a canned response";
        mock.addEventListener("click", () => {
          form.routes.push({
            method: op.route!.method,
            path: op.route!.path,
            status: "200",
            body: op.sampleBody,
          });
          for (const key of op.envKeys) {
            if (!form.env.some((e) => e.key === key)) {
              form.env.push({ key, value: key.includes("ENDPOINT") ? "$MOCK" : "demo" });
            }
          }
          changed(true);
        });
        head.append(mock);
        card.append(head);
      } else {
        const rm = document.createElement("button");
        rm.className = "row-remove";
        rm.textContent = "×";
        rm.title = "Stop mocking this operation";
        rm.addEventListener("click", () => {
          form.routes.splice(form.routes.indexOf(row), 1);
          changed(true);
        });
        head.append(rm);
        const statusLine = document.createElement("div");
        statusLine.className = "op-status-line";
        statusLine.append("responds", statusInput(row), "with");
        card.append(head, statusLine, bodyArea(row));
        issueNotes(card, form.routes.indexOf(row));
      }
      opRows.append(card);
    }

    /* Custom routes: whatever the declared operations do not cover. */
    routeRows.replaceChildren();
    form.routes.forEach((row, i) => {
      if (catalog.some((op) => op.route && op.route.method === row.method && op.route.path === row.path))
        return;
      const card = document.createElement("div");
      card.className = "route-card";
      const line = document.createElement("div");
      line.className = "route-line";
      const method = document.createElement("select");
      method.className = "route-method";
      for (const m of METHODS) {
        const opt = document.createElement("option");
        opt.value = m;
        opt.textContent = m;
        method.append(opt);
      }
      method.value = row.method;
      method.addEventListener("change", () => {
        row.method = method.value;
        changed(true);
      });
      const path = document.createElement("input");
      path.className = "route-path";
      path.placeholder = "/anything/else";
      path.value = row.path;
      path.addEventListener("input", () => {
        row.path = path.value;
        changed(false);
      });
      const rm = document.createElement("button");
      rm.className = "row-remove";
      rm.textContent = "×";
      rm.title = "Remove";
      rm.addEventListener("click", () => {
        form.routes.splice(i, 1);
        changed(true);
      });
      line.append(method, path, statusInput(row), rm);
      card.append(line, bodyArea(row));
      issueNotes(card, i);
      routeRows.append(card);
    });
    passthrough.checked = form.passthrough;
  }

  function changed(structural: boolean): void {
    if (structural) render();
    else {
      formEl.querySelectorAll(".field-error").forEach((el) => el.classList.remove("field-error"));
      formEl.querySelectorAll(".route-issue").forEach((el) => el.remove());
      const cards = [...formEl.querySelectorAll<HTMLElement>(".op-card, .route-card")];
      for (const card of cards) {
        const body = card.querySelector<HTMLTextAreaElement>(".route-body");
        if (!body) continue;
        const row = form.routes.find((r) => r.body === body.value);
        if (row) issueNotes(card, form.routes.indexOf(row));
      }
    }
    options.onChange();
  }

  $("#env-add").addEventListener("click", () => {
    form.env.push({ key: "", value: "" });
    changed(true);
  });
  $("#route-add").addEventListener("click", () => {
    form.routes.push({ method: "GET", path: "/", status: "200", body: "{}" });
    changed(true);
  });
  passthrough.addEventListener("change", () => {
    form.passthrough = passthrough.checked;
    changed(false);
  });

  modeBtn.addEventListener("click", () => {
    if (rawMode) {
      const parsed = formFromJson(options.rawEditor.get());
      if (!parsed) return; /* invalid JSON stays raw so nothing is lost */
      form = parsed;
      rawMode = false;
      render();
    } else {
      options.rawEditor.set(formToJson(form));
      rawMode = true;
    }
    formEl.hidden = rawMode;
    options.rawEditor.container.hidden = !rawMode;
    modeBtn.textContent = rawMode ? "edit as form" : "edit as JSON";
  });

  return {
    json: () => (rawMode ? options.rawEditor.get() : formToJson(form)),
    setJson: (text) => {
      const parsed = formFromJson(text);
      if (parsed && !rawMode) {
        form = parsed;
        render();
      } else {
        options.rawEditor.set(text);
        if (!rawMode) {
          rawMode = true;
          formEl.hidden = true;
          options.rawEditor.container.hidden = false;
          modeBtn.textContent = "edit as form";
        }
      }
    },
    setIr: (value) => {
      catalog = value ? opCatalog(value) : [];
      if (!rawMode) render();
    },
  };
}
