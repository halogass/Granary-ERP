/**
 * Links to a call. Staff use the member call page, which joins the room as themselves (the host when
 * they made it); /meet/<id> is the guest page for people without an account, who wait to be admitted.
 */
export function callPageUrl(roomId: string, audioOnly = false): string {
  const q = new URLSearchParams({ join: roomId });
  if (audioOnly) q.set("audio", "1");
  return `/calls?${q.toString()}`;
}

