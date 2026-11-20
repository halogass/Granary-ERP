"use client";

import { useMemo } from "react";
import { Combobox, Field, type ComboboxOption } from "@/components/ui";
import { orgSource, contactSource } from "@/lib/entitySources";

/**
 * Linked Organization + Contact pickers, both searchable.
 *
 * Contacts are scoped to the selected organization. If a contact is chosen while no
 * organization is set, the contact's own `organization_id` is written back to the org
 * field, so the pair can be filled from either direction.
 *
 * Renders two sibling Fields with no wrapper, so it drops into a FieldGrid.
 */
export default function OrgContactPicker({
  organizationId,
  contactId,
  onChange,
  orgLabel = "Organization",
  contactLabel = "Contact",
}: {
  organizationId: string;
  contactId: string;
  onChange: (next: { organization_id: string; contact_id: string }) => void;
  /** Kept for callers; the Field supplies the label style now. */
  labelClass?: string;
  orgLabel?: string;
  contactLabel?: string;
}) {
  const contacts = useMemo(() => contactSource(organizationId || undefined), [organizationId]);

  function pickContact(id: string, option: ComboboxOption | null) {
    // Contact chosen while no org is set: adopt the contact's own organization.
    const orgOf = typeof option?.meta?.organization_id === "string" ? option.meta.organization_id : "";
    onChange({
      organization_id: organizationId || orgOf,
      contact_id: id,
    });
  }

  return (
    <>
      <Field label={orgLabel}>
        <Combobox
          value={organizationId}
          source={orgSource}
          placeholder="Search organizations"
          emptyLabel="No organizations found"
          onChange={(id) => onChange({ organization_id: id, contact_id: "" })}
        />
      </Field>
      <Field label={contactLabel}>
        <Combobox
          value={contactId}
          source={contacts}
          placeholder={organizationId ? "Search contacts" : "Search all contacts"}
          emptyLabel={organizationId ? "No contacts at this organization" : "No contacts found"}
          onChange={pickContact}
        />
      </Field>
    </>
  );
}
