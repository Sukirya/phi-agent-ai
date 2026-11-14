// Workbench view trees: data the node validates and stores.
//
// The node owns validation (`phi-contracts/src/ui.rs`); these types mirror the
// protocol so the renderer can stay typed. Unknown kinds are rejected by the
// node, so the renderer only needs a fallback for forward compatibility.

export type ViewAction =
  | { kind: "message"; text: string }
  | { kind: "open_url"; url: string };

export type ViewNode = {
  kind: string;
  props?: Record<string, unknown>;
  children?: ViewNode[];
};

export type WorkbenchView = {
  version: number;
  title?: string;
  root: ViewNode;
};

export type Gap = "sm" | "md" | "lg";

export function asString(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

export function asNumber(value: unknown, fallback = 0): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

export function asArray<T>(value: unknown): T[] {
  return Array.isArray(value) ? (value as T[]) : [];
}

export function asGap(value: unknown): Gap {
  return value === "sm" || value === "lg" ? value : "md";
}

export function asAction(value: unknown): ViewAction | null {
  if (!value || typeof value !== "object") return null;
  const action = value as Record<string, unknown>;
  if (action.kind === "message" && typeof action.text === "string") {
    return { kind: "message", text: action.text };
  }
  if (action.kind === "open_url" && typeof action.url === "string") {
    const url = action.url;
    if (url.startsWith("https://") || url.startsWith("http://")) {
      return { kind: "open_url", url };
    }
  }
  return null;
}
