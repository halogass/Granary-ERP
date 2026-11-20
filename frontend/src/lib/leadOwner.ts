/**
 * Whether a lead is "mine" on the Leads page: the one it is assigned to, as the API's crm:view_own scope reads
 * it (routers/crm.rs list_leads filters by assigned_to_id), not the one who created it.
 */
export function isMyLead(lead: { assigned_to_id?: string | null }, myId: string | null | undefined): boolean {
  return !!myId && lead.assigned_to_id === myId;
}
