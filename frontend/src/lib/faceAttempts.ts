/**
 * The attempt log (migration 0165): every face check a clock runs reports how it ended. Fire and forget: the
 * report never delays or fails a clock-in.
 */
import type { FaceAttemptReport } from "@/lib/api";
import type { ChallengeResult } from "@/lib/faceChallenge";
import type { LivenessAction } from "@/lib/faceLiveness";

export function attemptReport(
  action: LivenessAction, result: ChallengeResult, backend: FaceAttemptReport["backend"], employeeId?: string,
): FaceAttemptReport {
  return {
    action,
    outcome: result.outcome,
    step_reached: result.step,
    duration_ms: Math.max(0, Math.min(600000, result.durationMs)),
    avg_tick_ms: result.avgTickMs === null ? null : Math.max(0, Math.min(60000, result.avgTickMs)),
    backend,
    ...(result.firstTickMs != null ? { first_tick_ms: Math.max(0, Math.min(600000, result.firstTickMs)) } : {}),
    ...(result.ticks != null ? { ticks: Math.max(0, Math.min(100000, result.ticks)) } : {}),
    ...(result.note ? { note: result.note.slice(0, 200) } : {}),
    ...(employeeId ? { employee_id: employeeId } : {}),
  };
}

/** Send without waiting and swallow every error. */
export function fireAndForget(send: () => Promise<unknown>): void {
  try {
    send().catch(() => {});
  } catch {
    /* never part of the clock flow */
  }
}

/** What the person is told when a check did not finish, by the step it reached. */
export function timeoutMessage(step: ChallengeResult["step"]): string {
  if (step === "find_face") return "No face was found in time. Face the camera in good light and try again.";
  if (step === "hold_still") return "Hold still a moment with your face in the oval, then try again.";
  if (step === "do_action") return "The movement was not seen in time. Try again, a little slower and bigger.";
  return "Face the camera again at the end. Try again.";
}
