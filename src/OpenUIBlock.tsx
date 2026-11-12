// React island for OpenUI Lang.
//
// The Tauri webview is SolidJS. The OpenUI renderer is a React component, so
// we load the prebuilt browser bundle (React is inlined) and mount the
// renderer into a plain DOM node. Only UI-mode assistant messages use this.

import { Show, createEffect, createSignal, onCleanup } from "solid-js";
import { t } from "./i18n";

type OpenUIGlobal = {
  React: any;
  createRoot: (node: Element) => { render: (node: unknown) => void; unmount: () => void };
  Renderer: any;
  openuiChatLibrary: any;
};

declare global {
  interface Window {
    __OpenUI?: OpenUIGlobal;
  }
}

let loader: Promise<OpenUIGlobal> | null = null;

async function loadOpenUI(): Promise<OpenUIGlobal> {
  if (window.__OpenUI) return window.__OpenUI;
  loader ??= (async () => {
    await import("@openuidev/browser-bundle");
    if (!window.__OpenUI) {
      throw new Error("OpenUI bundle did not initialise");
    }
    return window.__OpenUI;
  })();
  return loader;
}

/// Models occasionally wrap the program in a markdown fence despite the
/// system prompt. Accept both the raw program and a fenced one.
export function extractOpenUICode(text: string): string {
  const fenced = text.match(
    /```(?:openui(?:-lang)?|oui)\s*\r?\n([\s\S]*?)```/i,
  );
  if (fenced) return fenced[1].trim();
  const anyFence = text.match(/```\s*\r?\n([\s\S]*?)```/);
  if (anyFence && /(^|\n)\s*root\s*=/.test(anyFence[1])) {
    return anyFence[1].trim();
  }
  return text.trim();
}

export function OpenUIBlock(props: { code: string }) {
  let host!: HTMLDivElement;
  let root: ReturnType<OpenUIGlobal["createRoot"]> | undefined;
  const [failed, setFailed] = createSignal<string | null>(null);
  let disposed = false;

  const code = () => extractOpenUICode(props.code);

  createEffect(() => {
    const response = code();
    void render(response);
  });

  async function render(response: string) {
    if (!response) return;
    try {
      const ui = await loadOpenUI();
      if (disposed) return;
      root ??= ui.createRoot(host);
      root.render(
        ui.React.createElement(ui.Renderer, {
          response,
          library: ui.openuiChatLibrary,
        }),
      );
      setFailed(null);
    } catch (error) {
      setFailed(error instanceof Error ? error.message : String(error));
    }
  }

  onCleanup(() => {
    disposed = true;
    root?.unmount();
  });

  return (
    <div class="openui-wrap">
      <div class="openui" ref={host} />
      <Show when={failed()}>
        <div class="openui-error">{t("OpenUI could not render this answer; showing the raw program.")}</div>
        <pre class="openui-fallback">{code()}</pre>
      </Show>
    </div>
  );
}
