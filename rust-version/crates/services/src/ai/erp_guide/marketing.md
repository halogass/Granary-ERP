# Marketing: campaigns, content calendar, email marketing

Marketing plans campaigns, publishes content through approval, and sends email campaigns with a global unsubscribe.

- **Campaigns** (/marketing/campaigns): planned, active, paused, completed, cancelled.
- **Content calendar** (/marketing/content): planned, in_progress, review, approved, scheduled, published. "Approved" is never set by hand: scheduling or publishing passes the marketing_content approval (R&D lead for technical pieces, the PM for a customer reference, then the Owner).
- **Email marketing** (/marketing/email, lists and campaigns): campaigns draft → scheduled/sending → sent or failed, sent only from draft and only with lists attached. Limited to the marketing team and the Owner. Every email has the sender's signature footer and a one-click unsubscribe; unsubscribing adds the address to a global suppression list that every send respects (public page /unsubscribe).
