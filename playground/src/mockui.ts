/* The run environment editor. The visible form is env rows only: the one
   thing a run genuinely needs injected. Canned HTTP responses belong to the
   snippet (Go's httptest, an injected fetch in TypeScript) or, eventually, to
   the contract's conformance vectors; the raw mocks.json remains reachable
   behind "advanced" so old share links and route-table users keep working. */
import { emptyForm, formFromJson, formToJson, type MockForm } from "./mockform";

export interface MockUi {
  json(): string;
  setJson(text: string): void;
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

  let form: MockForm = emptyForm();
  let rawMode = false;

  function render(): void {
    envRows.replaceChildren();
    form.env.forEach((row, i) => {
      const line = document.createElement("div");
      line.className = "env-row";
      const key = document.createElement("input");
      key.className = "env-key";
      key.placeholder = "NAME";
      key.setAttribute("aria-label", "Environment variable name");
      key.value = row.key;
      key.addEventListener("input", () => {
        row.key = key.value;
        options.onChange();
      });
      const value = document.createElement("input");
      value.className = "env-value";
      value.placeholder = "value";
      value.setAttribute("aria-label", "Environment variable value");
      value.value = row.value;
      value.addEventListener("input", () => {
        row.value = value.value;
        options.onChange();
      });
      const rm = document.createElement("button");
      rm.className = "row-remove";
      rm.textContent = "×";
      rm.title = "Remove";
      rm.addEventListener("click", () => {
        form.env.splice(i, 1);
        render();
        options.onChange();
      });
      line.append(key, value, rm);
      envRows.append(line);
    });
  }

  $("#env-add").addEventListener("click", () => {
    form.env.push({ key: "", value: "" });
    render();
    options.onChange();
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
    modeBtn.textContent = rawMode ? "back to form" : "advanced";
    options.onChange();
  });

  return {
    json: () => (rawMode ? options.rawEditor.get() : formToJson(form)),
    setJson: (text) => {
      const parsed = formFromJson(text);
      /* Content the env-only form cannot represent (routes, passthrough)
         opens in the raw editor so nothing is silently dropped. */
      const fitsForm =
        parsed !== null && parsed.routes.length === 0 && !parsed.passthrough;
      if (fitsForm && !rawMode) {
        form = parsed;
        render();
      } else {
        options.rawEditor.set(text);
        if (!rawMode) {
          rawMode = true;
          formEl.hidden = true;
          options.rawEditor.container.hidden = false;
          modeBtn.textContent = "back to form";
        }
      }
    },
  };
}
