import { listen } from "@tauri-apps/api/event";

type ChirpState =
  | "idle"
  | "listening"
  | "thinking"
  | "acting"
  | "confirming"
  | "done"
  | "error";

const mascot = document.getElementById("mascot");

function render(state: ChirpState) {
  if (!mascot) return;
  mascot.setAttribute("data-state", state);
  // Restart the entry pop on every state change.
  mascot.classList.remove("pop");
  void mascot.offsetWidth;
  mascot.classList.add("pop");
}

// The overlay is driven only by the state machine in the Rust core.
listen<ChirpState>("chirp://state", (event) => render(event.payload));
