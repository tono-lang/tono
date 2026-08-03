/* The mock editor's DOM: renders the structured form, keeps a hidden JSON
   text in sync (the format runs and share links speak), and flips to the raw
   editor for hand editing. All conversion and validation logic lives in
   mockform.ts; this file is only wiring. */
import {
  emptyForm,
  formFromJson,
  formToJson,
  suggestFromIr,
  validate,
  METHODS,
  type MockForm,
} from "./mockform";

export interface MockUi {
  /* The current mocks.json text, whichever editor is active. */
  json(): string;
  /* Replace the content (a shared link restoring, or a template). */
  setJson(text: string): void;
  /* Provide the IR the "from ops" suggestion reads. */
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
  const routeRows = $("#route-rows");
  const passthrough = $<HTMLInputElement>("#mocks-passthrough");

  let form: MockForm = emptyForm();
  let rawMode = false;
  let ir: string | null = null;

  function render(): void {
    const issues = validate(form);
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

    routeRows.replaceChildren();
    form.routes.forEach((route, i) => {
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
      method.value = route.method;
      method.addEventListener("change", () => {
        route.method = method.value;
        changed(false);
      });
      const path = document.createElement("input");
      path.className = "route-path";
      path.placeholder = "/users/gandarfh";
      path.value = route.path;
      path.addEventListener("input", () => {
        route.path = path.value;
        changed(false);
      });
      const status = document.createElement("input");
      status.className = "route-status";
      status.value = route.status;
      status.title = "HTTP status";
      status.addEventListener("input", () => {
        route.status = status.value;
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
      line.append(method, path, status, rm);
      const body = document.createElement("textarea");
      body.className = "route-body";
      body.rows = 3;
      body.spellcheck = false;
      body.value = route.body;
      body.title = "Response body (JSON)";
      body.addEventListener("input", () => {
        route.body = body.value;
        changed(false);
      });
      card.append(line, body);
      for (const issue of issues.filter((x) => x.index === i)) {
        const el = { path, status, body }[issue.field];
        el.classList.add("field-error");
        const note = document.createElement("div");
        note.className = "route-issue";
        note.textContent = issue.message;
        card.append(note);
      }
      routeRows.append(card);
    });
    passthrough.checked = form.passthrough;
  }

  /* rerender only on structural changes; field edits keep focus. */
  function changed(structural: boolean): void {
    if (structural) render();
    else {
      /* refresh error marks in place without rebuilding inputs */
      const issues = validate(form);
      routeRows.querySelectorAll(".field-error").forEach((el) => el.classList.remove("field-error"));
      routeRows.querySelectorAll(".route-issue").forEach((el) => el.remove());
      issues.forEach((issue) => {
        const card = routeRows.children[issue.index];
        if (!card) return;
        const el = card.querySelector<HTMLElement>(
          issue.field === "path" ? ".route-path" : issue.field === "status" ? ".route-status" : ".route-body",
        );
        el?.classList.add("field-error");
        const note = document.createElement("div");
        note.className = "route-issue";
        note.textContent = issue.message;
        card.append(note);
      });
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
  $("#routes-suggest").addEventListener("click", () => {
    if (!ir) return;
    for (const s of suggestFromIr(ir)) {
      if (!form.routes.some((r) => r.method === s.method && r.path === s.path)) {
        form.routes.push({ method: s.method, path: s.path, status: "200", body: "{}" });
      }
      for (const key of s.envKeys) {
        if (!form.env.some((e) => e.key === key)) {
          form.env.push({ key, value: key.includes("ENDPOINT") ? "$MOCK" : "demo" });
        }
      }
    }
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
      ir = value;
    },
  };
}
