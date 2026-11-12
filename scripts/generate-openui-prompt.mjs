// Regenerate the OpenUI system prompt shipped with the Rust backend.
// The browser bundle renders the same `openuiChatLibrary`, so the model and
// the renderer always share one component contract.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  openuiChatLibrary,
  openuiChatPromptOptions,
} from "@openuidev/react-ui/genui-lib";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const output = resolve(
  root,
  "src-tauri/assets/openui-chat-system-prompt.txt",
);

const prompt = openuiChatLibrary.prompt(openuiChatPromptOptions);
mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, `${prompt}\n`, "utf8");
console.log(`wrote ${output} (${prompt.length} chars)`);
