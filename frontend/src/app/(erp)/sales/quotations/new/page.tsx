"use client";

import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import QuotationForm from "@/components/sales/QuotationForm";

function NewQuotation() {
  // "New quotation" on a deal opens this page with ?deal_id=…; the form pre-fills from the deal.
  const dealId = useSearchParams().get("deal_id") ?? undefined;
  return <QuotationForm mode="create" initialDealId={dealId} />;
}

export default function NewQuotationPage() {
  return (
    <Suspense fallback={null}>
      <NewQuotation />
    </Suspense>
  );
}
