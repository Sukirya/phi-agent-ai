// Solid component registry for workbench view trees.
//
// The node validates the protocol, so the renderer trusts `kind` to be known
// and only guards against malformed props. Every kind maps to a small host
// component; nothing here evaluates code or injects HTML.

import { For, Show, createSignal } from "solid-js";
import { t, tArgs } from "../i18n";
import {
  asAction,
  asArray,
  asGap,
  asNumber,
  asString,
  type ViewAction,
  type ViewNode,
  type WorkbenchView,
} from "./types";

type ActionHandler = (action: ViewAction) => void;

type NodeProps = {
  node: ViewNode;
  onAction: ActionHandler;
};

const GAP_CLASS: Record<string, string> = {
  sm: "gap-sm",
  md: "gap-md",
  lg: "gap-lg",
};

function children(node: ViewNode): ViewNode[] {
  return node.children ?? [];
}

export function ViewRenderer(props: {
  view: WorkbenchView;
  onAction: ActionHandler;
}) {
  return <NodeView node={props.view.root} onAction={props.onAction} />;
}

function NodeView(props: NodeProps) {
  switch (props.node.kind) {
    case "stack":
      return <Stack {...props} />;
    case "row":
      return <Row {...props} />;
    case "grid":
      return <Grid {...props} />;
    case "card":
      return <Card {...props} />;
    case "divider":
      return <hr class="workbench-divider" />;
    case "text":
      return <TextNode node={props.node} />;
    case "code":
      return <CodeNode node={props.node} />;
    case "table":
      return <TableNode node={props.node} />;
    case "keyvalue":
      return <KeyValueNode node={props.node} />;
    case "badge":
      return <BadgeNode node={props.node} />;
    case "progress":
      return <ProgressNode node={props.node} />;
    case "chart":
      return <ChartNode node={props.node} />;
    case "form":
      return <FormNode {...props} />;
    case "field":
      // A field outside a form has no submit target; show its label only.
      return (
        <div class="workbench-field">
          <span class="workbench-field-label">
            {asString(props.node.props?.label, asString(props.node.props?.name))}
          </span>
        </div>
      );
    case "button":
      return <ButtonNode node={props.node} onAction={props.onAction} />;
    default:
      return (
        <div class="workbench-unsupported">
          {tArgs("Unsupported view component: {kind}", { kind: props.node.kind })}
        </div>
      );
  }
}

function Stack(props: NodeProps) {
  return (
    <div class={`workbench-stack ${GAP_CLASS[asGap(props.node.props?.gap)]}`}>
      <For each={children(props.node)}>
        {(child) => <NodeView node={child} onAction={props.onAction} />}
      </For>
    </div>
  );
}

function Row(props: NodeProps) {
  const wrap = () => props.node.props?.wrap === true;
  return (
    <div
      classList={{
        "workbench-row": true,
        [GAP_CLASS[asGap(props.node.props?.gap)]]: true,
        wrap: wrap(),
      }}
    >
      <For each={children(props.node)}>
        {(child) => <NodeView node={child} onAction={props.onAction} />}
      </For>
    </div>
  );
}

function Grid(props: NodeProps) {
  const columns = () => Math.min(4, Math.max(1, Math.round(asNumber(props.node.props?.columns, 2))));
  return (
    <div class="workbench-grid" style={{ "--grid-columns": String(columns()) }}>
      <For each={children(props.node)}>
        {(child) => <NodeView node={child} onAction={props.onAction} />}
      </For>
    </div>
  );
}

function Card(props: NodeProps) {
  const title = () => asString(props.node.props?.title);
  const subtitle = () => asString(props.node.props?.subtitle);
  return (
    <section class="workbench-card">
      <Show when={title() || subtitle()}>
        <header class="workbench-card-head">
          <Show when={title()}>
            <div class="workbench-card-title">{title()}</div>
          </Show>
          <Show when={subtitle()}>
            <div class="workbench-card-subtitle">{subtitle()}</div>
          </Show>
        </header>
      </Show>
      <div class="workbench-card-body">
        <For each={children(props.node)}>
          {(child) => <NodeView node={child} onAction={props.onAction} />}
        </For>
      </div>
    </section>
  );
}

function TextNode(props: { node: ViewNode }) {
  return <div class="workbench-text">{asString(props.node.props?.text)}</div>;
}

function CodeNode(props: { node: ViewNode }) {
  const language = () => asString(props.node.props?.language);
  return (
    <div class="workbench-code-block">
      <Show when={language()}>
        <div class="workbench-code-lang">{language()}</div>
      </Show>
      <pre class="workbench-code">{asString(props.node.props?.code)}</pre>
    </div>
  );
}

function TableNode(props: { node: ViewNode }) {
  const columns = () => asArray<unknown>(props.node.props?.columns).map(String);
  const rows = () => asArray<unknown[]>(props.node.props?.rows);
  return (
    <div class="workbench-table-wrap">
      <table class="workbench-table">
        <thead>
          <tr>
            <For each={columns()}>{(column) => <th>{column}</th>}</For>
          </tr>
        </thead>
        <tbody>
          <For each={rows()}>
            {(row) => (
              <tr>
                <For each={columns()}>
                  {(_, index) => <td>{formatCell(row[index()])}</td>}
                </For>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
}

function formatCell(value: unknown): string {
  if (value === null || value === undefined) return "";
  return typeof value === "object" ? JSON.stringify(value) : String(value);
}

function KeyValueNode(props: { node: ViewNode }) {
  const items = () => asArray<Record<string, unknown>>(props.node.props?.items);
  return (
    <dl class="workbench-keyvalue">
      <For each={items()}>
        {(item) => (
          <>
            <dt>{asString(item.label)}</dt>
            <dd>{formatCell(item.value)}</dd>
          </>
        )}
      </For>
    </dl>
  );
}

const BADGE_TONES = ["neutral", "ok", "warn", "error", "info"];

function BadgeNode(props: { node: ViewNode }) {
  const tone = () => {
    const value = asString(props.node.props?.tone, "neutral");
    return BADGE_TONES.includes(value) ? value : "neutral";
  };
  return (
    <span class={`workbench-badge tone-${tone()}`}>
      {asString(props.node.props?.text)}
    </span>
  );
}

function ProgressNode(props: { node: ViewNode }) {
  const value = () => Math.min(100, Math.max(0, asNumber(props.node.props?.value)));
  const label = () => asString(props.node.props?.label);
  return (
    <div class="workbench-progress">
      <Show when={label()}>
        <div class="workbench-progress-head">
          <span>{label()}</span>
          <span class="workbench-progress-value">{Math.round(value())}%</span>
        </div>
      </Show>
      <div class="workbench-progress-track">
        <div class="workbench-progress-fill" style={{ width: `${value()}%` }} />
      </div>
    </div>
  );
}

const SERIES_COLORS = ["#6ea8fe", "#7ee787", "#e3b341", "#ff7b72", "#c297ff", "#39c5cf"];

function ChartNode(props: { node: ViewNode }) {
  const type = () => (props.node.props?.type === "line" ? "line" : "bar");
  const labels = () => asArray<unknown>(props.node.props?.labels).map(String);
  const series = () =>
    asArray<Record<string, unknown>>(props.node.props?.series).map((entry, index) => ({
      name: asString(entry.name),
      values: asArray<unknown>(entry.values).map((value) => asNumber(value)),
      color: SERIES_COLORS[index % SERIES_COLORS.length],
    }));
  const height = () => Math.min(480, Math.max(120, asNumber(props.node.props?.height, 220)));
  const max = () => {
    let top = 0;
    for (const line of series()) {
      for (const value of line.values) {
        if (value > top) top = value;
      }
    }
    return top > 0 ? top : 1;
  };

  const geometry = () => {
    const width = 640;
    const boxHeight = height();
    const left = 8;
    const top = 8;
    const plotWidth = width - left * 2;
    const plotHeight = boxHeight - top - 26;
    return { width, boxHeight, left, top, plotWidth, plotHeight };
  };

  const barRects = () => {
    const { left, top, plotWidth, plotHeight } = geometry();
    const rows = labels();
    const lines = series();
    if (rows.length === 0 || lines.length === 0) return [];
    const slot = plotWidth / rows.length;
    const barWidth = Math.max(2, (slot * 0.7) / lines.length);
    const rects: {
      x: number;
      y: number;
      width: number;
      height: number;
      fill: string;
    }[] = [];
    rows.forEach((_, rowIndex) => {
      const groupLeft = left + rowIndex * slot + slot * 0.15;
      lines.forEach((line, lineIndex) => {
        const value = line.values[rowIndex] ?? 0;
        const barHeight = (value / max()) * plotHeight;
        rects.push({
          x: groupLeft + lineIndex * barWidth,
          y: top + plotHeight - barHeight,
          width: barWidth * 0.9,
          height: barHeight,
          fill: line.color,
        });
      });
    });
    return rects;
  };

  const linePaths = () => {
    const { left, top, plotWidth, plotHeight } = geometry();
    const count = labels().length;
    return series().map((line) => {
      const points = line.values.slice(0, count).map((value, index) => {
        const x = count <= 1 ? left + plotWidth / 2 : left + (index / (count - 1)) * plotWidth;
        const y = top + plotHeight - (value / max()) * plotHeight;
        return `${x.toFixed(1)},${y.toFixed(1)}`;
      });
      return { points: points.join(" "), color: line.color, name: line.name };
    });
  };

  const labelTexts = () => {
    const { left, top, plotWidth, plotHeight } = geometry();
    const rows = labels();
    const slot = rows.length > 0 ? plotWidth / rows.length : plotWidth;
    return rows.map((label, index) => ({
      x: left + index * slot + slot / 2,
      y: top + plotHeight + 18,
      text: label.length > 14 ? `${label.slice(0, 13)}…` : label,
    }));
  };

  return (
    <div class="workbench-chart">
      <svg
        viewBox={`0 0 ${geometry().width} ${geometry().boxHeight}`}
        role="img"
        preserveAspectRatio="xMidYMid meet"
      >
        <line
          x1={geometry().left}
          y1={geometry().top + geometry().plotHeight}
          x2={geometry().width - geometry().left}
          y2={geometry().top + geometry().plotHeight}
          stroke="currentColor"
          opacity="0.25"
        />
        <Show when={type() === "bar"}>
          <For each={barRects()}>
            {(rect) => (
              <rect
                x={rect.x}
                y={rect.y}
                width={rect.width}
                height={rect.height}
                rx="2"
                fill={rect.fill}
              />
            )}
          </For>
        </Show>
        <Show when={type() === "line"}>
          <For each={linePaths()}>
            {(line) => (
              <polyline
                points={line.points}
                fill="none"
                stroke={line.color}
                stroke-width="2"
                stroke-linejoin="round"
              />
            )}
          </For>
        </Show>
        <For each={labelTexts()}>
          {(label) => (
            <text x={label.x} y={label.y} text-anchor="middle" font-size="11" fill="currentColor" opacity="0.7">
              {label.text}
            </text>
          )}
        </For>
      </svg>
      <div class="workbench-legend">
        <For each={series()}>
          {(line) => (
            <span class="workbench-legend-item">
              <span class="workbench-legend-swatch" style={{ background: line.color }} />
              {line.name}
            </span>
          )}
        </For>
      </div>
    </div>
  );
}

function FormNode(props: NodeProps) {
  const fields = () =>
    children(props.node).filter((child) => child.kind === "field");
  const initial = () => {
    const values: Record<string, string> = {};
    for (const field of fields()) {
      const name = asString(field.props?.name);
      if (name) values[name] = asString(field.props?.value);
    }
    return values;
  };
  const [values, setValues] = createSignal<Record<string, string>>(initial());

  function update(name: string, value: string) {
    setValues({ ...values(), [name]: value });
  }

  function submit(event: Event) {
    event.preventDefault();
    const lines: string[] = [];
    for (const field of fields()) {
      const name = asString(field.props?.name);
      if (!name) continue;
      lines.push(`${asString(field.props?.label, name)}: ${values()[name] ?? ""}`);
    }
    if (lines.length > 0) {
      props.onAction({ kind: "message", text: lines.join("\n") });
    }
  }

  return (
    <form
      class="workbench-form"
      onSubmit={submit}
    >
      <For each={fields()}>
        {(field) => (
          <FieldControl
            node={field}
            value={values()[asString(field.props?.name)] ?? ""}
            onChange={update}
          />
        )}
      </For>
      <button type="submit" class="primary workbench-form-submit">
        {asString(props.node.props?.submitLabel, t("Send"))}
      </button>
    </form>
  );
}

function FieldControl(props: {
  node: ViewNode;
  value: string;
  onChange: (name: string, value: string) => void;
}) {
  const name = () => asString(props.node.props?.name);
  const label = () => asString(props.node.props?.label, name());
  const hint = () => asString(props.node.props?.hint);
  const input = () => {
    const kind = asString(props.node.props?.input, "text");
    return kind === "number" || kind === "select" ? kind : "text";
  };
  return (
    <label class="workbench-field">
      <span class="workbench-field-label">{label()}</span>
      <Show
        when={input() === "select"}
        fallback={
          <input
            type={input() === "number" ? "number" : "text"}
            value={props.value}
            onInput={(event) =>
              props.onChange(name(), event.currentTarget.value)
            }
          />
        }
      >
        <select
          value={props.value}
          onChange={(event) =>
            props.onChange(name(), event.currentTarget.value)
          }
        >
          <For each={asArray<unknown>(props.node.props?.options).map(String)}>
            {(option) => (
              <option value={option} selected={option === props.value}>
                {option}
              </option>
            )}
          </For>
        </select>
      </Show>
      <Show when={hint()}>
        <span class="workbench-field-hint">{hint()}</span>
      </Show>
    </label>
  );
}

function ButtonNode(props: { node: ViewNode; onAction: ActionHandler }) {
  const action = () => asAction(props.node.props?.action);
  const variant = () => {
    const value = asString(props.node.props?.variant, "default");
    return value === "primary" || value === "danger" ? value : "default";
  };
  return (
    <button
      classList={{
        "workbench-button": true,
        primary: variant() === "primary",
        danger: variant() === "danger",
      }}
      disabled={!action()}
      onClick={() => {
        const next = action();
        if (next) props.onAction(next);
      }}
    >
      {asString(props.node.props?.label)}
    </button>
  );
}
