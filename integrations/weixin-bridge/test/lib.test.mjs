import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

import { ThreadStore } from "../../bridge-core/src/lib.mjs";

import {
  activeTurnBlock,
  commandAction,
  extractText,
  MessageItemType,
  parseBool,
  parseCommand,
  parseList,
  preservedChatStateFields,
  processUpdateBatch,
  splitMessage
} from "../src/lib.mjs";

test("extractText reads text and voice transcript items", () => {
  assert.equal(
    extractText([{ type: MessageItemType.TEXT, text_item: { text: "hello" } }]),
    "hello"
  );
  assert.equal(
    extractText([{ type: MessageItemType.VOICE, voice_item: { text: "voice text" } }]),
    "voice text"
  );
});

test("shared command helpers preserve Weixin bridge command behavior", () => {
  assert.deepEqual(parseList("u1, u2 ,, "), ["u1", "u2"]);
  assert.equal(parseBool("yes"), true);
  assert.deepEqual(parseCommand("/allow ap_1 remember"), {
    name: "allow",
    args: "ap_1 remember"
  });
  assert.deepEqual(commandAction(parseCommand("/model auto")), {
    kind: "set_model",
    modelName: "auto"
  });
  assert.deepEqual(commandAction(parseCommand("/unknown value")), {
    kind: "prompt",
    prompt: "/unknown value"
  });
});

test("shared state and runtime helpers preserve Weixin bridge behavior", () => {
  assert.deepEqual(preservedChatStateFields({ model: "m", activeTurnId: "turn-1" }), {
    model: "m"
  });
  assert.deepEqual(splitMessage("a🧪b", 2), ["a🧪", "b"]);
  assert.deepEqual(activeTurnBlock({ turns: [{ id: "turn-1", status: "in_progress" }] }), {
    turnId: "turn-1",
    message: "Thread already has active turn turn-1. Wait for it to finish or send /interrupt."
  });
});

test("a crash mid-batch replays the batch without losing or re-running prompts", async () => {
  const dir = await mkdtemp(path.join(tmpdir(), "codewhale-weixin-bridge-"));
  try {
    const statePath = path.join(dir, "thread-map.json");
    const batch = [1, 2, 3].map((id) => ({ from_user_id: "u", message_id: id }));
    const keyOf = (msg) => `${msg.from_user_id}:${msg.message_id}`;
    const handled = [];
    const cursors = [];
    const crash = new Error("process killed");
    const first = await ThreadStore.open(statePath, { messageLimit: 50 });
    let afterCrash = null;
    await assert.rejects(processUpdateBatch({
      messages: batch, nextCursor: "buf-2", store: first, keyOf,
      handle: async (msg) => {
        if (msg.message_id === 2) {
          // What a restarted process finds on disk while message 2 runs.
          afterCrash = await ThreadStore.open(statePath, { messageLimit: 50 });
          throw crash;
        }
        handled.push(msg.message_id);
      },
      interrupted: async () => assert.fail("nothing is interrupted on the first pass"),
      commitCursor: async (buf) => cursors.push(buf),
    }), crash);
    assert.deepEqual(cursors, [], "the cursor does not move past unhandled messages");

    const reported = [];
    await processUpdateBatch({
      messages: batch, nextCursor: "buf-2", store: afterCrash, keyOf,
      handle: async (msg) => handled.push(msg.message_id),
      interrupted: async (msg) => reported.push(msg.message_id),
      commitCursor: async (buf) => cursors.push(buf),
    });
    assert.deepEqual(handled, [1, 3], "message 1 is not re-run; message 3 is not lost");
    assert.deepEqual(reported, [2], "the in-flight message is reported, not re-run");
    assert.deepEqual(cursors, ["buf-2"]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
