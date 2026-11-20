/**
 * Project task statuses and priorities, one list for every screen (the project page, My Tasks).
 * The values match what the API and the board store: "cancelled" (not "blocked") and "critical" (not "urgent").
 */
export const TASK_STATUS_LABELS: Record<string, string> = {
  todo: "To do", in_progress: "In progress", review: "In review", done: "Done", cancelled: "Cancelled",
};

export const TASK_PRIORITY_LABELS: Record<string, string> = {
  low: "Low", medium: "Medium", high: "High", critical: "Critical",
};
