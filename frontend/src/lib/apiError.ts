// Extracts a human-readable message from an Axios error raised by the Granary API.
//
// The backend validation handler (backend/app/core/exceptions.py) returns:
//   { detail: "Validation failed", errors: [{ field, message }], code: "VALIDATION_ERROR" }
// Other handlers return: { detail: "<message>", code: "..." }
// Workflow gates (approvals) return: { detail: { code, message, messages? } }
//
// Prefer the per-field `errors[]` so the user sees exactly which field failed,
// then fall back to `detail`, then to the provided fallback string.

interface StructuredDetail {
  code?: string;
  message?: string;
  messages?: string[];
}

interface ApiErrorData {
  detail?: string | StructuredDetail;
  errors?: { field?: string; message?: string }[];
}

export function extractApiError(err: unknown, fallback = "Something went wrong. Please try again."): string {
  if (err && typeof err === "object" && "response" in err) {
    const data = (err as { response?: { data?: ApiErrorData } }).response?.data;
    if (data?.errors?.length) {
      return data.errors
        .map((e) => (e.field ? `${e.field}: ${e.message}` : e.message))
        .filter(Boolean)
        .join("; ");
    }
    if (typeof data?.detail === "string" && data.detail) return data.detail;
    if (data?.detail && typeof data.detail === "object") {
      if (data.detail.messages?.length) return data.detail.messages.join("; ");
      if (data.detail.message) return data.detail.message;
    }
  }
  return fallback;
}

/** The machine-readable `detail.code` of a structured API error (e.g. "approval_pending"), if any. */
export function apiErrorCode(err: unknown): string | undefined {
  if (err && typeof err === "object" && "response" in err) {
    const detail = (err as { response?: { data?: ApiErrorData } }).response?.data?.detail;
    if (detail && typeof detail === "object") return detail.code;
  }
  return undefined;
}
