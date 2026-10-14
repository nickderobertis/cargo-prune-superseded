// Analyze the release journey's recording; onebudgetspec owns result validation.
import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

const recording = JSON.parse(readFileSync(
  new URL("../../../../dist/prune-workload/telemetry/prune-pass.json", import.meta.url),
  "utf8",
));
report(
  recording.value,
  `generated entries: ${recording.generated_entries}; generated profiles: ${recording.generated_profiles}; ${recording.detail}`,
);
