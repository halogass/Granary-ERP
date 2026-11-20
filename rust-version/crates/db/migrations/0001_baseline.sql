-- Granary: the database baseline. One file creates the whole schema and the default data that every install
-- starts from (settings defaults, permissions, approval policies, document categories, the assistant's autonomy
-- levels and roles). The application seed (crates/db/src/seed) runs on every start and adds what is missing.
-- New schema changes go in 0002_..., 0003_... after this file.

CREATE EXTENSION IF NOT EXISTS vector WITH SCHEMA public;

COMMENT ON EXTENSION vector IS 'vector data type and ivfflat and hnsw access methods';

CREATE FUNCTION public.production_serial_key(s text) RETURNS text
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    AS $$ SELECT translate(regexp_replace(upper(s), '[-[:space:]]', '', 'g'), 'ILO', '110') $$;

CREATE TABLE public.accounts (
    code character varying(20) NOT NULL,
    name character varying(200) NOT NULL,
    category character varying(20) NOT NULL,
    account_type character varying(50),
    description text,
    is_active boolean,
    normal_balance character varying(6),
    currency character varying(3),
    allow_direct_posting boolean,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.activities (
    entity_type character varying(50) NOT NULL,
    entity_id uuid NOT NULL,
    contact_id uuid,
    lead_id uuid,
    deal_id uuid,
    activity_type character varying(50) NOT NULL,
    direction character varying(20),
    subject character varying(500),
    content text,
    outcome character varying(255),
    duration_minutes integer,
    scheduled_at timestamp with time zone,
    completed_at timestamp with time zone,
    is_completed boolean,
    next_action character varying(500),
    next_action_due date,
    channel_message_id character varying(255),
    channel_data jsonb,
    created_by_id uuid,
    assigned_to_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean DEFAULT false NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.activities_voided (
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    activity_id uuid NOT NULL,
    snapshot jsonb NOT NULL,
    reason text,
    voided_by_id uuid,
    voided_by_name character varying(255),
    restored_at timestamp with time zone,
    restored_by_id uuid
);

CREATE TABLE public.agent_pending_actions (
    action character varying(60) NOT NULL,
    origin character varying(20) DEFAULT 'nary'::character varying NOT NULL,
    status character varying(20) DEFAULT 'pending'::character varying NOT NULL,
    level integer NOT NULL,
    customer_facing boolean DEFAULT false NOT NULL,
    payload jsonb NOT NULL,
    edited_payload jsonb,
    rationale text,
    sop_ref character varying(40),
    sources jsonb,
    diff jsonb,
    price jsonb,
    entity_type character varying(40),
    entity_id uuid,
    run_id uuid,
    actor_id uuid,
    assignee_id uuid,
    decided_by_id uuid,
    decided_at timestamp with time zone,
    reject_reason text,
    edit_ratio numeric(6,2),
    result jsonb,
    error text,
    actual_text text,
    diff_ratio numeric(6,2),
    compared_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    trigger_key character varying(160)
);

CREATE TABLE public.agent_runs (
    trigger_type character varying(20) NOT NULL,
    trigger_ref character varying(80),
    job character varying(60) NOT NULL,
    context character varying(80) DEFAULT 'internal'::character varying NOT NULL,
    sop_ref character varying(40),
    model character varying(80),
    prompt_tokens integer DEFAULT 0 NOT NULL,
    completion_tokens integer DEFAULT 0 NOT NULL,
    cache_read_tokens integer DEFAULT 0 NOT NULL,
    cache_write_tokens integer DEFAULT 0 NOT NULL,
    est_cost numeric(12,6) DEFAULT '0'::numeric NOT NULL,
    status character varying(20) DEFAULT 'running'::character varying NOT NULL,
    outcome text,
    error text,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    finished_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.agent_steps (
    run_id uuid NOT NULL,
    seq integer NOT NULL,
    kind character varying(20) NOT NULL,
    name character varying(80),
    input jsonb,
    output jsonb,
    prompt_tokens integer DEFAULT 0 NOT NULL,
    completion_tokens integer DEFAULT 0 NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.ai_conversations (
    user_id uuid NOT NULL,
    title character varying(300),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.ai_knowledge_chunks (
    doc_id uuid NOT NULL,
    chunk_index integer NOT NULL,
    content text NOT NULL,
    embedding public.vector(768),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.ai_knowledge_docs (
    title character varying(300) NOT NULL,
    description text,
    source_type character varying(10) NOT NULL,
    content text,
    original_filename character varying(300),
    original_object_key character varying(500),
    original_size integer,
    status character varying(20) NOT NULL,
    error text,
    is_active boolean NOT NULL,
    customer_safe boolean DEFAULT false NOT NULL,
    chunk_count integer NOT NULL,
    use_count integer NOT NULL,
    last_used_at timestamp with time zone,
    created_by_id uuid,
    system_key character varying(120),
    content_hash character varying(64),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.ai_messages (
    conversation_id uuid NOT NULL,
    role character varying(20) NOT NULL,
    content text,
    tool_calls jsonb,
    citations jsonb,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.ai_tool_jobs (
    id uuid NOT NULL,
    user_id uuid,
    tool character varying(80) NOT NULL,
    status character varying(20) DEFAULT 'running'::character varying NOT NULL,
    result jsonb,
    error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    finished_at timestamp with time zone
);

CREATE TABLE public.ai_usage_log (
    user_id uuid,
    feature character varying(40) NOT NULL,
    provider character varying(30),
    model character varying(80),
    prompt_tokens integer,
    completion_tokens integer,
    cache_read_tokens integer DEFAULT 0,
    cache_write_1h_tokens integer DEFAULT 0 NOT NULL,
    cache_write_tokens integer DEFAULT 0,
    est_cost numeric(12,6),
    cost_known boolean DEFAULT true NOT NULL,
    run_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.applicants (
    job_opening_id uuid NOT NULL,
    name character varying(255) NOT NULL,
    email character varying(255),
    phone character varying(50),
    whatsapp character varying(50),
    linkedin_url character varying(255),
    portfolio_url character varying(255),
    stage character varying(50),
    resume_url character varying(500),
    cover_letter text,
    source character varying(100),
    referred_by_id uuid,
    rating integer,
    notes text,
    rejection_reason character varying(255),
    assigned_to_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.approval_actions (
    request_id uuid NOT NULL,
    step_seq integer,
    actor_id uuid,
    action character varying(20) NOT NULL,
    comment text,
    ip_address character varying(45),
    user_agent text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.approval_policies (
    entity_type character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    sop_reference character varying(100),
    is_active boolean NOT NULL,
    version integer NOT NULL,
    preconditions jsonb NOT NULL,
    signature_flow character varying(40) NOT NULL,
    signer_role character varying(100),
    sign_deadline_days integer,
    max_extension_days integer,
    qr_enabled boolean NOT NULL,
    view_scope character varying(30) NOT NULL,
    allow_download boolean NOT NULL,
    requires_login boolean NOT NULL,
    token_ttl_days integer,
    sla_hours integer,
    whatsapp_escalate_above_base numeric(18,2),
    updated_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.approval_policy_steps (
    policy_id uuid NOT NULL,
    sequence integer NOT NULL,
    name character varying(255) NOT NULL,
    approver_mode character varying(20) NOT NULL,
    role_slug character varying(100),
    department character varying(100),
    snapshot_user_key character varying(100),
    user_id uuid,
    condition jsonb,
    sla_hours integer,
    is_notify_only boolean NOT NULL,
    allow_self_approval boolean NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.approval_requests (
    entity_type character varying(50) NOT NULL,
    entity_id uuid NOT NULL,
    policy_id uuid,
    policy_version integer,
    status character varying(20) NOT NULL,
    document_revision integer NOT NULL,
    document_label character varying(255),
    submitted_by_id uuid,
    submitted_at timestamp with time zone NOT NULL,
    completed_at timestamp with time zone,
    current_step_seq integer,
    snapshot jsonb NOT NULL,
    flags jsonb NOT NULL,
    steps jsonb NOT NULL,
    last_reminded_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.asset_categories (
    name character varying(100) NOT NULL,
    description text,
    depreciation_method character varying(30),
    useful_life_months integer,
    salvage_value_pct numeric(5,2),
    gl_asset_account_id uuid,
    gl_depreciation_account_id uuid,
    gl_accumulated_account_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.asset_depreciation_lines (
    asset_id uuid NOT NULL,
    period_date date NOT NULL,
    period_label character varying(20),
    depreciation_amount numeric(18,2) NOT NULL,
    accumulated_depreciation numeric(18,2) NOT NULL,
    book_value numeric(18,2) NOT NULL,
    journal_entry_id uuid,
    status character varying(20),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.asset_disposals (
    asset_id uuid NOT NULL,
    disposal_date date NOT NULL,
    disposal_type character varying(30) NOT NULL,
    proceeds numeric(18,2),
    bank_account_id uuid,
    disposal_notes text,
    journal_entry_id uuid,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.attachments (
    entity_type character varying(30) NOT NULL,
    entity_id character varying(36) NOT NULL,
    uploaded_by_id uuid,
    file_name character varying(255) NOT NULL,
    file_size integer NOT NULL,
    mime_type character varying(100) NOT NULL,
    object_key character varying(500) NOT NULL,
    url character varying(500) NOT NULL,
    file_type character varying(20) NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    uploaded_via character varying(20),
    duration_seconds double precision
);

CREATE TABLE public.attendance_records (
    employee_id uuid NOT NULL,
    date date NOT NULL,
    status character varying(30),
    check_in character varying(10),
    check_out character varying(10),
    work_hours numeric(4,2),
    late_minutes integer,
    overtime_hours numeric(4,2),
    notes text,
    recorded_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.audit_logs (
    user_id uuid,
    user_name character varying(255),
    action character varying(50) NOT NULL,
    entity_type character varying(100) NOT NULL,
    entity_id character varying(100),
    entity_label character varying(200),
    old_values json,
    new_values json,
    changed_fields character varying[],
    ip_address character varying(45),
    user_agent character varying(500),
    notes text,
    origin character varying(20) DEFAULT 'ui'::character varying NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.bank_accounts (
    name character varying(200) NOT NULL,
    bank_name character varying(100) NOT NULL,
    account_number character varying(100),
    currency character varying(3),
    gl_account_id uuid,
    current_balance numeric(18,2),
    is_active boolean,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.bank_statement_lines (
    statement_id uuid NOT NULL,
    transaction_date date NOT NULL,
    description character varying(500),
    reference character varying(200),
    debit numeric(18,2),
    credit numeric(18,2),
    balance numeric(18,2),
    is_matched boolean,
    is_excluded boolean DEFAULT false NOT NULL,
    matched_journal_line_id uuid,
    matched_at timestamp with time zone,
    matched_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.bank_statements (
    bank_account_id uuid NOT NULL,
    period_start date,
    period_end date,
    opening_balance numeric(18,2),
    closing_balance numeric(18,2),
    pdf_url character varying(500),
    status character varying(20),
    notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.board_columns (
    scope_type character varying(20) NOT NULL,
    scope_id character varying(36) NOT NULL,
    key character varying(40) NOT NULL,
    label character varying(60) NOT NULL,
    color character varying(16),
    sort_order integer,
    wip_limit integer,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.bom_items (
    bom_id uuid NOT NULL,
    component_name character varying(255) NOT NULL,
    component_sku character varying(50),
    description text,
    category character varying(100),
    quantity numeric(10,3) NOT NULL,
    unit character varying(20),
    unit_cost numeric(18,4),
    cost_currency character varying(3),
    lead_time_days integer,
    supplier_name character varying(255),
    supplier_sku character varying(100),
    is_critical boolean,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.boms (
    product_id uuid NOT NULL,
    version character varying(20) NOT NULL,
    status character varying(20),
    is_active boolean,
    notes text,
    change_reason text,
    approved_by_id uuid,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.budget_lines (
    budget_id uuid NOT NULL,
    account_id uuid NOT NULL,
    label character varying(255),
    budgeted_amount numeric(18,2) NOT NULL,
    currency character varying(3),
    period_month integer,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.budgets (
    name character varying(255) NOT NULL,
    fiscal_year integer NOT NULL,
    department character varying(100),
    project_id uuid,
    period_type character varying(20),
    status character varying(20),
    approved_by_id uuid,
    approved_at timestamp with time zone,
    notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.catalog_prices (
    product_id uuid NOT NULL,
    currency character varying(3) NOT NULL,
    amount numeric(18,2) NOT NULL,
    valid_from date NOT NULL,
    valid_to date,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.catalog_volume_tiers (
    product_id uuid NOT NULL,
    min_qty numeric(10,2) NOT NULL,
    max_qty numeric(10,2),
    discount_pct numeric(6,3) NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.channel_memberships (
    channel_id uuid NOT NULL,
    user_id uuid NOT NULL,
    role character varying(20),
    last_read_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.channel_messages (
    channel_id uuid NOT NULL,
    sender_id uuid NOT NULL,
    content text NOT NULL,
    message_type character varying(20),
    file_url character varying(500),
    file_name character varying(255),
    file_size integer,
    mime_type character varying(100),
    reply_to_id uuid,
    thread_root_id uuid,
    thread_count integer,
    mentions jsonb,
    is_deleted boolean,
    memo_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.channels (
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description text,
    type character varying(20),
    created_by_id uuid NOT NULL,
    is_archived boolean,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.clock_events (
    employee_id uuid,
    event_type character varying(20) NOT NULL,
    captured_at timestamp with time zone NOT NULL,
    recognition_status character varying(30) NOT NULL,
    recognition_uid character varying(100),
    recognition_confidence double precision,
    liveness_score double precision,
    is_real_face boolean,
    latitude numeric(10,7),
    longitude numeric(10,7),
    geo_accuracy_meters double precision,
    office_location_id uuid,
    geo_distance_meters double precision,
    override_by_id uuid,
    override_at timestamp with time zone,
    override_notes text,
    attendance_record_id uuid,
    is_kiosk boolean,
    kiosk_device_id uuid,
    liveness_challenge character varying(20),
    liveness_passed boolean,
    match_distance double precision,
    details jsonb,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.component_inventory (
    component_sku character varying(50) NOT NULL,
    component_name character varying(255) NOT NULL,
    category character varying(100),
    quantity_on_hand numeric(10,3),
    quantity_reserved numeric(10,3),
    quantity_available numeric(10,3),
    unit character varying(20),
    unit_cost numeric(18,4),
    cost_currency character varying(3),
    safety_stock numeric(10,3),
    reorder_point numeric(10,3),
    reorder_quantity numeric(10,3),
    location character varying(100),
    location_id uuid,
    primary_supplier character varying(255),
    last_purchase_date date,
    last_purchase_price numeric(18,4),
    notes text,
    updated_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.contacts (
    organization_id uuid,
    first_name character varying(100) NOT NULL,
    last_name character varying(100),
    email character varying(255),
    phone character varying(50),
    whatsapp character varying(50),
    "position" character varying(150),
    department character varying(100),
    is_primary boolean,
    linkedin_url character varying(255),
    country character varying(100),
    notes text,
    tags character varying[],
    assigned_to_id uuid,
    created_by_id uuid,
    custom jsonb DEFAULT '{}'::jsonb NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.content_calendar (
    campaign_id uuid,
    title character varying(500) NOT NULL,
    content_type character varying(50),
    platform character varying(50),
    status character varying(30),
    planned_date date,
    published_at timestamp with time zone,
    content_body text,
    media_urls character varying[],
    external_url character varying(500),
    assigned_to_id uuid,
    approved_by_id uuid,
    views integer,
    clicks integer,
    leads_from_content integer,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.contractor_requests (
    requested_by_id uuid NOT NULL,
    department character varying(100),
    vendor_type character varying(50),
    scope_description text NOT NULL,
    estimated_value numeric(18,2),
    currency character varying(3),
    needed_by date,
    status character varying(30),
    approved_by_id uuid,
    approved_at timestamp with time zone,
    rejection_reason text,
    preferred_vendor_id uuid,
    selected_vendor_id uuid,
    work_order_id uuid,
    purchase_order_id uuid,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.contracts (
    number character varying(50) NOT NULL,
    quotation_id uuid,
    deal_id uuid,
    customer_org_id uuid,
    customer_contact_id uuid,
    status character varying(30),
    contract_type character varying(50),
    value numeric(18,2),
    currency character varying(3),
    start_date date,
    end_date date,
    auto_renew boolean,
    signed_at timestamp with time zone,
    signed_by_customer_name character varying(255),
    signed_by_customer_title character varying(255),
    signed_by_company_id uuid,
    signed_by_company_name character varying(255),
    document_url character varying(500),
    document_id character varying(100),
    notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.crm_ai_field_runs (
    object_type character varying(20) NOT NULL,
    record_id uuid NOT NULL,
    attribute_key character varying(64) NOT NULL,
    value jsonb,
    confidence numeric(5,2),
    reasoning text,
    sources jsonb,
    provider character varying(30),
    model character varying(80),
    prompt_tokens integer NOT NULL,
    completion_tokens integer NOT NULL,
    error text,
    run_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.crm_attribute_defs (
    object_type character varying(20) NOT NULL,
    key character varying(64) NOT NULL,
    label character varying(200) NOT NULL,
    description text,
    field_type character varying(24) NOT NULL,
    options jsonb,
    is_ai boolean NOT NULL,
    ai_kind character varying(20),
    ai_config jsonb,
    sort_order integer NOT NULL,
    is_active boolean NOT NULL,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.crm_duplicate_candidates (
    object_type character varying(20) NOT NULL,
    record_a_id uuid NOT NULL,
    record_b_id uuid NOT NULL,
    score numeric(5,2) NOT NULL,
    reasons jsonb,
    status character varying(20) NOT NULL,
    resolved_by_id uuid,
    resolved_at timestamp with time zone,
    merged_into_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.crm_email_attachments (
    message_id uuid NOT NULL,
    filename character varying(500) NOT NULL,
    content_type character varying(200),
    size_bytes integer,
    data bytea NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.crm_email_messages (
    thread_id uuid NOT NULL,
    message_id_header character varying(1000) NOT NULL,
    in_reply_to character varying(1000),
    "references" text,
    direction character varying(10) NOT NULL,
    from_email character varying(500) NOT NULL,
    from_name character varying(500),
    to_emails character varying[],
    cc_emails character varying[],
    subject character varying(1000),
    body_html text,
    body_text text,
    imap_uid integer,
    imap_folder character varying(40),
    reply_to character varying(500),
    auth jsonb,
    is_read boolean DEFAULT true NOT NULL,
    received_at timestamp with time zone,
    sent_by_id uuid,
    nary_processed_at timestamp with time zone,
    sent_by_nary boolean DEFAULT false NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    mailbox_id uuid,
    origin character varying(10) DEFAULT 'live'::character varying NOT NULL,
    card_id uuid,
    bcc_emails character varying[]
);

CREATE TABLE public.crm_email_threads (
    subject character varying(1000),
    status character varying(20),
    is_read boolean,
    message_count integer,
    last_sender_email character varying(500),
    last_sender_name character varying(500),
    last_message_at timestamp with time zone,
    linked_contact_id uuid,
    linked_org_id uuid,
    linked_lead_id uuid,
    linked_deal_id uuid,
    assigned_to_id uuid,
    assigned_by_id uuid,
    assigned_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    mailbox_id uuid
);

CREATE TABLE public.crm_embeddings (
    object_type character varying(30) NOT NULL,
    record_id uuid NOT NULL,
    label text NOT NULL,
    embedding public.vector(768),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.crm_mailboxes (
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    prefix character varying(40) NOT NULL,
    address character varying(255) NOT NULL,
    display_name character varying(120) DEFAULT ''::character varying NOT NULL,
    purpose character varying(20) DEFAULT 'general'::character varying NOT NULL,
    signature_html text,
    roles jsonb DEFAULT '[]'::jsonb NOT NULL,
    active boolean DEFAULT true NOT NULL,
    is_default boolean DEFAULT false NOT NULL,
    sort integer DEFAULT 0 NOT NULL,
    connected_at timestamp with time zone,
    backfill_days integer DEFAULT 90 NOT NULL,
    backfill_done boolean DEFAULT false NOT NULL
);

CREATE TABLE public.crm_message_copies (
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    message_id uuid NOT NULL,
    mailbox_id uuid NOT NULL,
    folder character varying(40) NOT NULL,
    imap_uid integer NOT NULL,
    is_read boolean DEFAULT false NOT NULL
);

CREATE TABLE public.crm_reminders (
    owner_user_id uuid NOT NULL,
    created_by_id uuid,
    source character varying(20) NOT NULL,
    entity_type character varying(30) NOT NULL,
    entity_id uuid,
    chat_id character varying(255),
    title character varying(255) NOT NULL,
    note text,
    due_at timestamp with time zone NOT NULL,
    original_due_at timestamp with time zone,
    status character varying(20) NOT NULL,
    snooze_count integer NOT NULL,
    completed_at timestamp with time zone,
    completed_by_id uuid,
    notified_at timestamp with time zone,
    escalated_at timestamp with time zone,
    rule_key character varying(255),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.crm_research_people (
    organization_id uuid NOT NULL,
    run_id uuid,
    name character varying(200) NOT NULL,
    name_key character varying(200) NOT NULL,
    title character varying(200),
    source_url character varying(1000),
    evidence text,
    status character varying(20) NOT NULL,
    contact_id uuid,
    matched_contact_id uuid,
    resolved_by_id uuid,
    resolved_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.crm_research_runs (
    object_type character varying(20) NOT NULL,
    record_id uuid NOT NULL,
    depth character varying(10) NOT NULL,
    status character varying(20) NOT NULL,
    summary text,
    sources jsonb,
    news jsonb,
    channels jsonb,
    written jsonb,
    suggested jsonb,
    error text,
    requested_by_id uuid,
    started_at timestamp with time zone,
    finished_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.deal_stage_events (
    deal_id uuid NOT NULL,
    from_stage character varying(50),
    to_stage character varying(50) NOT NULL,
    at timestamp with time zone DEFAULT now() NOT NULL,
    by_id uuid,
    origin character varying(30) NOT NULL,
    reasons jsonb,
    note text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.deals (
    lead_id uuid,
    contact_id uuid,
    organization_id uuid,
    title character varying(255) NOT NULL,
    deal_number character varying(50),
    stage character varying(50) DEFAULT 'new'::character varying NOT NULL,
    value numeric(18,2),
    currency character varying(3),
    probability integer,
    expected_close_date date,
    actual_close_date date,
    product_interests character varying[],
    product_line_ids character varying[],
    notes text,
    loss_reason character varying(255),
    loss_reasons jsonb,
    loss_notes text,
    assigned_to_id uuid,
    created_by_id uuid,
    deal_registration_id uuid,
    custom jsonb DEFAULT '{}'::jsonb NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone,
    mailbox_id uuid,
    segment character varying(32)
);

CREATE TABLE public.dept_coordinative_links (
    from_department character varying(100) NOT NULL,
    to_department character varying(100) NOT NULL,
    notes character varying(255),
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.direct_messages (
    thread_id character varying(100) NOT NULL,
    sender_id uuid NOT NULL,
    receiver_id uuid NOT NULL,
    content text NOT NULL,
    message_type character varying(20),
    file_url character varying(500),
    file_name character varying(255),
    file_size integer,
    mime_type character varying(100),
    is_read boolean,
    read_at timestamp with time zone,
    is_deleted_by_sender boolean,
    is_deleted_by_receiver boolean,
    reply_to_id uuid,
    thread_root_id uuid,
    thread_count integer,
    mentions jsonb,
    memo_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.discount_allowances (
    kind character varying(30) NOT NULL,
    label character varying(255) NOT NULL,
    min_qty numeric(10,2),
    max_qty numeric(10,2),
    max_discount_pct numeric(5,2) NOT NULL,
    notify_owner boolean NOT NULL,
    is_active boolean NOT NULL,
    sop_reference character varying(100),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.doc_categories (
    id uuid NOT NULL,
    key character varying(40) NOT NULL,
    name character varying(120) NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    visible_roles text[] DEFAULT '{}'::text[] NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.document_sequences (
    sequence_type character varying(50) NOT NULL,
    prefix character varying(30) NOT NULL,
    suffix_separator character varying(5),
    year integer NOT NULL,
    month integer NOT NULL,
    current_sequence integer NOT NULL,
    sequence_padding integer,
    last_generated character varying(100),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.document_verification_views (
    verification_id uuid NOT NULL,
    user_id uuid,
    ip_address character varying(45),
    user_agent text,
    outcome character varying(30) NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.document_verifications (
    entity_type character varying(50) NOT NULL,
    entity_id uuid NOT NULL,
    revision integer NOT NULL,
    document_number character varying(100),
    document_title character varying(255),
    token uuid NOT NULL,
    short_code character varying(16) NOT NULL,
    status character varying(20) NOT NULL,
    issued_at timestamp with time zone NOT NULL,
    expires_at timestamp with time zone,
    revoked_at timestamp with time zone,
    revoked_by_id uuid,
    revoke_reason text,
    superseded_by_id uuid,
    view_scope character varying(30) NOT NULL,
    allow_download boolean NOT NULL,
    requires_login boolean NOT NULL,
    document_hash character varying(64),
    pdf_object_key character varying(500),
    rendered_at timestamp with time zone,
    facts jsonb NOT NULL,
    approval_request_id uuid,
    sign_document_id uuid,
    issued_by_id uuid,
    view_count integer NOT NULL,
    last_viewed_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    sealed_object_key character varying(500),
    sealed_hash character varying(64),
    sealed_at timestamp with time zone,
    seal_level character varying(20)
);

CREATE TABLE public.domain_events (
    type character varying(60) NOT NULL,
    entity_type character varying(40),
    entity_id uuid,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    actor_id uuid,
    occurred_at timestamp with time zone DEFAULT now() NOT NULL,
    dispatched_at timestamp with time zone,
    attempts integer DEFAULT 0 NOT NULL,
    last_error text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    quiet_done_at timestamp with time zone
);

CREATE TABLE public.email_accounts (
    user_id uuid NOT NULL,
    email_address character varying(255) NOT NULL,
    display_name character varying(255),
    imap_host character varying(255) NOT NULL,
    imap_port integer,
    imap_use_ssl boolean,
    smtp_host character varying(255) NOT NULL,
    smtp_port integer,
    smtp_use_tls boolean,
    username character varying(255) NOT NULL,
    password_encrypted text NOT NULL,
    is_active boolean,
    is_default boolean,
    sync_enabled boolean,
    last_synced_at timestamp with time zone,
    last_sync_error text,
    sync_folder character varying(100),
    last_uid_synced integer,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.email_campaigns (
    name character varying(255) NOT NULL,
    subject character varying(500) NOT NULL,
    from_name character varying(255),
    from_email character varying(255),
    blocks jsonb,
    status character varying(30),
    scheduled_at timestamp with time zone,
    sent_at timestamp with time zone,
    list_ids jsonb,
    total_recipients integer,
    sent_count integer,
    failed_count integer,
    open_count integer,
    created_by_id uuid,
    sender_user_id uuid,
    include_signature boolean DEFAULT true NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.email_image_senders (
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    user_id uuid NOT NULL,
    sender_email character varying(320) NOT NULL
);

CREATE TABLE public.email_messages (
    account_id uuid NOT NULL,
    message_id character varying(500) NOT NULL,
    imap_uid integer,
    folder character varying(100),
    subject character varying(1000),
    from_address character varying(500),
    to_addresses character varying[],
    cc_addresses character varying[],
    reply_to character varying(500),
    in_reply_to character varying(500),
    thread_id character varying(500),
    body_text text,
    body_html text,
    is_read boolean,
    is_starred boolean,
    is_draft boolean,
    is_sent boolean,
    is_archived boolean,
    is_deleted boolean,
    date_received timestamp with time zone,
    date_sent timestamp with time zone,
    attachments jsonb,
    linked_contact_id uuid,
    linked_deal_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.email_queue_items (
    campaign_id uuid NOT NULL,
    list_id uuid NOT NULL,
    contact_id uuid NOT NULL,
    email character varying(255) NOT NULL,
    first_name character varying(100),
    full_name character varying(255),
    company character varying(255),
    tracking_id character varying(36) NOT NULL,
    status character varying(20) NOT NULL,
    error_message character varying(500),
    queued_at timestamp with time zone DEFAULT now(),
    sent_at timestamp with time zone,
    opened_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.email_signature_assets (
    filename character varying(255) NOT NULL,
    mime_type character varying(100) NOT NULL,
    object_key character varying(500) NOT NULL,
    file_size integer,
    source_url character varying(500),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.email_suppressions (
    email character varying(255) NOT NULL,
    reason character varying(30) NOT NULL,
    campaign_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.employee_allowances (
    employee_id uuid NOT NULL,
    name character varying(100) NOT NULL,
    amount numeric(18,2) NOT NULL,
    currency character varying(3) NOT NULL,
    is_fixed boolean NOT NULL,
    is_taxable boolean NOT NULL,
    is_active boolean NOT NULL,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.employees (
    user_id uuid,
    employee_number character varying(50),
    first_name character varying(100) NOT NULL,
    last_name character varying(100),
    email character varying(255) NOT NULL,
    personal_email character varying(255),
    phone character varying(50),
    whatsapp character varying(50),
    department character varying(100),
    "position" character varying(150),
    employment_type character varying(30),
    status character varying(30),
    reports_to_id uuid,
    start_date date,
    probation_end_date date,
    end_date date,
    termination_reason character varying(255),
    base_salary numeric(18,2),
    salary_currency character varying(3),
    salary_effective_date date,
    id_type character varying(20),
    id_number character varying(50),
    tax_id character varying(50),
    health_insurance_number character varying(50),
    social_security_number character varying(50),
    address text,
    city character varying(100),
    province character varying(100),
    emergency_contact_name character varying(255),
    emergency_contact_phone character varying(50),
    emergency_contact_relation character varying(50),
    notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone,
    tax_status character varying(30)
);

CREATE TABLE public.epics (
    scope_type character varying(20) NOT NULL,
    scope_id character varying(36) NOT NULL,
    title character varying(300) NOT NULL,
    description text,
    color character varying(16),
    status character varying(20),
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.exchange_rates (
    from_currency character varying(3) NOT NULL,
    to_currency character varying(3) NOT NULL,
    rate numeric(24,12) NOT NULL,
    effective_date date NOT NULL,
    source character varying(50),
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.expenses (
    category character varying(100) NOT NULL,
    description character varying(500) NOT NULL,
    amount numeric(18,2) NOT NULL,
    currency character varying(3),
    exchange_rate numeric(18,6),
    expense_date date NOT NULL,
    project_id uuid,
    vendor_id uuid,
    work_order_id uuid,
    status character varying(30),
    receipt_url character varying(500),
    invoice_number character varying(100),
    tax_deductible boolean,
    withholding_applicable boolean,
    withholding_rate numeric(6,3),
    withholding_amount numeric(18,2),
    submitted_by_id uuid,
    employee_id uuid,
    reimbursed_at timestamp with time zone,
    bank_account_id uuid,
    approved_by_id uuid,
    approved_at timestamp with time zone,
    rejection_reason text,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.face_check_attempts (
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    mode character varying(10) NOT NULL,
    kiosk_device_id uuid,
    user_id uuid,
    employee_id uuid,
    action character varying(20) NOT NULL,
    outcome character varying(30) NOT NULL,
    step_reached character varying(20) NOT NULL,
    duration_ms integer NOT NULL,
    avg_tick_ms integer,
    backend character varying(10) DEFAULT 'unknown'::character varying NOT NULL,
    user_agent character varying(200),
    note text,
    details jsonb,
    CONSTRAINT face_check_attempts_action_check CHECK (((action)::text = ANY (ARRAY[('blink'::character varying)::text, ('turn_left'::character varying)::text, ('turn_right'::character varying)::text]))),
    CONSTRAINT face_check_attempts_backend_check CHECK (((backend)::text = ANY (ARRAY[('webgl'::character varying)::text, ('cpu'::character varying)::text, ('unknown'::character varying)::text]))),
    CONSTRAINT face_check_attempts_mode_check CHECK (((mode)::text = ANY (ARRAY[('kiosk'::character varying)::text, ('personal'::character varying)::text]))),
    CONSTRAINT face_check_attempts_outcome_check CHECK (((outcome)::text = ANY (ARRAY[('done'::character varying)::text, ('timeout'::character varying)::text, ('aborted'::character varying)::text, ('passive_inconclusive'::character varying)::text, ('passive_failed'::character varying)::text])))
);

CREATE TABLE public.face_descriptors (
    employee_id uuid NOT NULL,
    descriptor double precision[] NOT NULL,
    model character varying(40) NOT NULL,
    enrolled_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.face_enrollments (
    employee_id uuid NOT NULL,
    face_uid character varying(100),
    enrolled_at timestamp with time zone,
    enrolled_by_id uuid,
    is_active boolean,
    image_count integer,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.fiscal_periods (
    year integer NOT NULL,
    month integer NOT NULL,
    name character varying(50),
    start_date date NOT NULL,
    end_date date NOT NULL,
    is_locked boolean,
    locked_by_id uuid,
    locked_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.fixed_assets (
    number character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    category_id uuid,
    status character varying(30),
    purchase_date date NOT NULL,
    purchase_price numeric(18,2) NOT NULL,
    currency character varying(3),
    acquisition_rate numeric(24,12),
    useful_life_months integer,
    salvage_value numeric(18,2),
    depreciation_method character varying(30),
    location character varying(200),
    serial_number character varying(100),
    vendor_id uuid,
    purchase_order_id uuid,
    notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.fleet_assignments (
    vehicle_id uuid NOT NULL,
    employee_id uuid,
    start_date date NOT NULL,
    end_date date,
    purpose character varying(255),
    notes text,
    assigned_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.fleet_fuel_logs (
    vehicle_id uuid NOT NULL,
    log_date date NOT NULL,
    odometer_reading numeric(10,1),
    fuel_liters numeric(8,2) NOT NULL,
    fuel_cost numeric(14,2) NOT NULL,
    currency character varying(3),
    fuel_station character varying(255),
    logged_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.fleet_odometer_logs (
    vehicle_id uuid NOT NULL,
    log_date date NOT NULL,
    odometer_reading numeric(10,1) NOT NULL,
    notes text,
    logged_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.fleet_service_records (
    vehicle_id uuid NOT NULL,
    service_date date NOT NULL,
    service_type character varying(30) NOT NULL,
    description text,
    cost numeric(14,2),
    currency character varying(3),
    vendor_name character varying(255),
    odometer_reading numeric(10,1),
    next_service_date date,
    next_service_odometer numeric(10,1),
    logged_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.fleet_vehicles (
    plate_number character varying(20) NOT NULL,
    make character varying(100) NOT NULL,
    model character varying(100) NOT NULL,
    year integer NOT NULL,
    color character varying(50),
    vehicle_type character varying(30),
    fuel_type character varying(20),
    status character varying(20),
    purchase_date date,
    purchase_price numeric(18,2),
    currency character varying(3),
    insurance_policy character varying(100),
    insurance_expiry date,
    registration_expiry date,
    odometer_current numeric(10,1),
    notes text,
    image_url character varying(500),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.gitlab_configs (
    name character varying(200) NOT NULL,
    url character varying(500) NOT NULL,
    api_version character varying(8),
    access_token_encrypted text,
    webhook_secret character varying(200),
    is_active boolean,
    verify_ssl boolean,
    instance_name character varying(200),
    instance_version character varying(50),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.gitlab_issue_queue (
    repository_id uuid NOT NULL,
    issue_iid integer NOT NULL,
    queued_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.gitlab_pipeline_runs (
    repository_id uuid NOT NULL,
    gitlab_pipeline_id integer NOT NULL,
    status character varying(32) NOT NULL,
    ref character varying(200),
    sha character varying(40),
    source character varying(50),
    triggered_by character varying(200),
    web_url character varying(500),
    started_at timestamp with time zone,
    finished_at timestamp with time zone,
    duration_seconds integer,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.gitlab_repositories (
    gitlab_config_id uuid NOT NULL,
    rnd_project_id uuid,
    gitlab_project_id integer NOT NULL,
    full_path character varying(500) NOT NULL,
    name character varying(200) NOT NULL,
    description text,
    default_branch character varying(100),
    web_url character varying(500),
    webhook_id integer,
    last_pipeline_status character varying(32),
    last_pipeline_ref character varying(200),
    last_pipeline_at timestamp with time zone,
    is_active boolean,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    issue_sync boolean DEFAULT false NOT NULL,
    issue_sync_labels_at timestamp with time zone,
    issue_sync_imported_at timestamp with time zone,
    issue_sync_pulled_at timestamp with time zone,
    issue_sync_at timestamp with time zone,
    issue_sync_error text
);

CREATE TABLE public.gitlab_user_links (
    id uuid NOT NULL,
    gitlab_config_id uuid NOT NULL,
    user_id uuid,
    gitlab_user_id bigint,
    gitlab_username character varying(255),
    matched_by character varying(10) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.gitlab_webhook_events (
    repository_id uuid NOT NULL,
    event_type character varying(50) NOT NULL,
    object_id integer,
    object_iid integer,
    object_title character varying(500),
    object_state character varying(50),
    object_action character varying(50),
    author_name character varying(200),
    author_username character varying(200),
    web_url character varying(500),
    linked_backlog_item_id uuid,
    raw_payload jsonb,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.gl_account_roles (
    role character varying(100) NOT NULL,
    account_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.goods_receipts (
    receipt_number character varying(50),
    po_id uuid NOT NULL,
    received_by_id uuid NOT NULL,
    received_at timestamp with time zone NOT NULL,
    received_date date,
    status character varying(20),
    location_id uuid,
    items_received jsonb,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.google_accounts (
    email character varying(255) NOT NULL,
    access_token text,
    refresh_token text,
    expires_at timestamp with time zone,
    scopes text,
    status character varying(20) DEFAULT 'connected'::character varying NOT NULL,
    last_error text,
    connected_by_id uuid,
    oauth_state character varying(64),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.hardware_products (
    sku character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    category character varying(50),
    product_line character varying(100),
    product_line_id uuid,
    hardware_version character varying(20),
    has_firmware boolean,
    has_software boolean,
    unit_cost numeric(18,4),
    currency character varying(3),
    current_bom_id uuid,
    is_active boolean,
    image_url character varying(500),
    notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone,
    serial_prefix character varying(6),
    CONSTRAINT ck_hardware_products_serial_prefix CHECK (((serial_prefix IS NULL) OR ((serial_prefix)::text ~ '^[A-Z]{2,6}$'::text)))
);

CREATE TABLE public.helpdesk_categories (
    team_id uuid NOT NULL,
    name character varying(100) NOT NULL,
    description text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.helpdesk_messages (
    ticket_id uuid NOT NULL,
    body text NOT NULL,
    is_internal boolean NOT NULL,
    author_name character varying(255),
    author_email character varying(255),
    author_user_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.helpdesk_sla_policies (
    team_id uuid NOT NULL,
    name character varying(100) NOT NULL,
    priority character varying(20) NOT NULL,
    response_hours numeric(5,1) NOT NULL,
    resolution_hours numeric(5,1) NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.helpdesk_teams (
    name character varying(100) NOT NULL,
    email character varying(255),
    description text,
    is_active boolean NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.helpdesk_tickets (
    number character varying(50) NOT NULL,
    subject character varying(500) NOT NULL,
    description text,
    status character varying(30),
    priority character varying(20),
    team_id uuid,
    category_id uuid,
    sla_policy_id uuid,
    customer_name character varying(255) NOT NULL,
    customer_email character varying(255) NOT NULL,
    customer_company character varying(255),
    assignee_id uuid,
    public_token uuid NOT NULL,
    source character varying(30),
    response_deadline timestamp with time zone,
    resolution_deadline timestamp with time zone,
    first_responded_at timestamp with time zone,
    resolved_at timestamp with time zone,
    closed_at timestamp with time zone,
    first_response_breached boolean,
    resolution_breached boolean,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.hiring_requests (
    title character varying(255) NOT NULL,
    department character varying(100) NOT NULL,
    employment_type character varying(30),
    requested_by_id uuid NOT NULL,
    reports_to_employee_id uuid,
    budget_approved boolean,
    approved_by_id uuid,
    approved_at timestamp with time zone,
    status character varying(30),
    urgency character varying(20),
    target_start_date date,
    headcount integer,
    job_description text,
    requirements text,
    salary_budget_min numeric(18,2),
    salary_budget_max numeric(18,2),
    salary_currency character varying(3),
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.invoice_items (
    invoice_id uuid NOT NULL,
    product_id uuid,
    sort_order integer,
    sku character varying(50),
    name character varying(255) NOT NULL,
    description text,
    quantity numeric(10,2) NOT NULL,
    unit character varying(30),
    unit_price numeric(18,2) NOT NULL,
    currency character varying(3),
    discount_pct numeric(5,2) NOT NULL,
    discount_amount numeric(18,2) NOT NULL,
    amount numeric(18,2) NOT NULL,
    tax_id uuid,
    tax_code character varying(50),
    tax_pct numeric(8,4) NOT NULL,
    tax_amount numeric(18,2) NOT NULL,
    image_url character varying(500),
    parent_item_id uuid,
    term_months integer,
    price_rule character varying(30),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.invoice_refunds (
    invoice_id uuid NOT NULL,
    amount numeric(18,2) NOT NULL,
    currency character varying(3) NOT NULL,
    exchange_rate numeric(18,6) NOT NULL,
    paid_date date NOT NULL,
    bank_account_id uuid,
    reference character varying(255),
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.invoices (
    number character varying(50) NOT NULL,
    invoice_type character varying(30),
    revision integer,
    full_number character varying(80),
    revision_group character varying(50),
    previous_version_id uuid,
    revision_reason text,
    credit_of_invoice_id uuid,
    deal_id uuid,
    project_id uuid,
    quotation_id uuid,
    ref_pi_number character varying(50),
    customer_org_id uuid,
    customer_contact_id uuid,
    customer_name character varying(255),
    customer_address text,
    customer_tax_id character varying(100),
    customer_tel character varying(50),
    customer_email character varying(255),
    customer_contact_person character varying(255),
    customer_contact_email character varying(255),
    status character varying(30),
    currency character varying(3),
    exchange_rate numeric(18,6),
    subtotal numeric(18,2),
    tax_lines jsonb,
    tax_total numeric(18,2),
    total numeric(18,2),
    amount_paid numeric(18,2),
    amount_due numeric(18,2),
    refund_due numeric(18,2) DEFAULT '0'::numeric NOT NULL,
    issue_date date NOT NULL,
    due_date date,
    paid_at timestamp with time zone,
    payment_method character varying(100),
    payment_reference character varying(255),
    payment_terms_text text,
    incoterms character varying(20),
    incoterms_destination character varying(255),
    carrier character varying(100),
    tracking_number character varying(100),
    hs_code character varying(50),
    country_of_origin character varying(100),
    shipped_from character varying(100),
    pdf_url character varying(500),
    authorized_by_id uuid,
    authorized_by_name character varying(255),
    authorized_by_title character varying(255),
    notes text,
    internal_notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.issue_activity (
    entity_type character varying(40) NOT NULL,
    entity_id character varying(36) NOT NULL,
    actor_id uuid,
    verb character varying(20) NOT NULL,
    field character varying(60),
    old_value character varying(500),
    new_value character varying(500),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.issue_comments (
    entity_type character varying(40) NOT NULL,
    entity_id character varying(36) NOT NULL,
    author_id uuid,
    body text NOT NULL,
    mentions jsonb,
    edited_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    gitlab_note_id bigint,
    origin character varying(10),
    gitlab_author character varying(255),
    gitlab_sync_error text
);

CREATE TABLE public.job_openings (
    hiring_request_id uuid,
    title character varying(255) NOT NULL,
    department character varying(100),
    employment_type character varying(30),
    status character varying(30),
    description text,
    requirements text,
    responsibilities text,
    benefits text,
    salary_min numeric(18,2),
    salary_max numeric(18,2),
    salary_currency character varying(3),
    show_salary boolean,
    location character varying(100),
    is_remote boolean,
    published_at timestamp with time zone,
    closing_date date,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.journal_entries (
    entry_number character varying(50) NOT NULL,
    entry_date date NOT NULL,
    fiscal_period_id uuid,
    narration text NOT NULL,
    reference character varying(200),
    status character varying(20),
    source character varying(50),
    source_id character varying(100),
    currency character varying(3),
    exchange_rate numeric(18,6),
    posted_by_id uuid,
    posted_at timestamp with time zone,
    reversed_by_id uuid,
    reversed_at timestamp with time zone,
    reversal_entry_id uuid,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.journal_lines (
    entry_id uuid NOT NULL,
    account_id uuid NOT NULL,
    description character varying(500),
    debit numeric(18,2) NOT NULL,
    credit numeric(18,2) NOT NULL,
    currency character varying(3),
    amount_currency numeric(18,2),
    sort_order integer,
    bank_statement_line_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.kb_article_revisions (
    article_id uuid NOT NULL,
    version character varying(20) NOT NULL,
    content_snapshot text,
    change_summary text,
    changed_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.kb_articles (
    category_id uuid,
    title character varying(500) NOT NULL,
    slug character varying(500) NOT NULL,
    content text,
    summary text,
    article_type character varying(30),
    sop_code character varying(50),
    sop_owner_role character varying(100),
    sop_trigger text,
    sop_steps jsonb,
    status character varying(30),
    version character varying(20),
    previous_version_id uuid,
    version_notes text,
    access_level character varying(30),
    is_private boolean DEFAULT false NOT NULL,
    tags character varying[],
    related_article_ids uuid[],
    view_count integer,
    helpful_votes integer,
    unhelpful_votes integer,
    created_by_id uuid,
    reviewed_by_id uuid,
    published_by_id uuid,
    author_signed_at timestamp with time zone,
    author_signature_url character varying(500),
    owner_signed_by_id uuid,
    owner_signed_at timestamp with time zone,
    owner_signature_url character varying(500),
    source_pdf_attachment_id uuid,
    pdf_import_status character varying(20),
    pdf_import_error text,
    pdf_import_mode character varying(20),
    pdf_import_pages integer,
    pdf_import_truncated boolean,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.kb_categories (
    name character varying(255) NOT NULL,
    slug character varying(255) NOT NULL,
    description text,
    parent_id uuid,
    sort_order integer,
    icon character varying(100),
    access_level character varying(30),
    allowed_departments character varying[],
    allowed_roles character varying[],
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.kb_embeddings (
    article_id uuid NOT NULL,
    chunk_index integer NOT NULL,
    content text NOT NULL,
    embedding public.vector(768),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.kiosk_devices (
    name character varying(100) NOT NULL,
    office_location_id uuid NOT NULL,
    token_hash character varying(64) NOT NULL,
    token_prefix character varying(12) NOT NULL,
    registered_by_id uuid,
    last_seen_at timestamp with time zone,
    revoked_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.lead_attributions (
    lead_id uuid,
    campaign_id uuid,
    content_item_id uuid,
    utm_source character varying(100),
    utm_medium character varying(100),
    utm_campaign character varying(100),
    utm_content character varying(100),
    utm_term character varying(100),
    referrer_url character varying(500),
    landing_page character varying(500),
    attribution_type character varying(30),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.leads (
    contact_id uuid,
    organization_id uuid,
    source character varying(50) NOT NULL,
    source_detail character varying(255),
    source_campaign_id uuid,
    partner_id uuid,
    title character varying(255),
    description text,
    estimated_value numeric(18,2),
    currency character varying(3),
    interested_products character varying[],
    status character varying(50),
    disqualification_reason character varying(255),
    priority character varying(20),
    assigned_to_id uuid,
    created_by_id uuid,
    converted_to_deal_id uuid,
    converted_at timestamp with time zone,
    raw_message text,
    raw_channel_data jsonb,
    attribution jsonb,
    ai_score jsonb,
    custom jsonb DEFAULT '{}'::jsonb NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone,
    segment character varying(32)
);

CREATE TABLE public.leave_requests (
    employee_id uuid,
    submitted_by_id uuid,
    leave_type character varying(50) NOT NULL,
    start_date date NOT NULL,
    end_date date NOT NULL,
    days numeric(4,1) NOT NULL,
    half_day boolean,
    half_day_period character varying(10),
    status character varying(30),
    reason text,
    approved_by_id uuid,
    approved_at timestamp with time zone,
    rejection_reason text,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.library_article_links (
    article_id uuid NOT NULL,
    document_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.library_document_products (
    id uuid NOT NULL,
    document_id uuid NOT NULL,
    product_id uuid,
    family character varying(50),
    CONSTRAINT ck_library_document_products_one CHECK (((product_id IS NULL) <> (family IS NULL)))
);

CREATE TABLE public.library_document_versions (
    id uuid NOT NULL,
    document_id uuid NOT NULL,
    label character varying(60) NOT NULL,
    change_note text NOT NULL,
    status character varying(20) DEFAULT 'draft'::character varying NOT NULL,
    file_object_key character varying(500) NOT NULL,
    file_name character varying(255) NOT NULL,
    file_mime character varying(120) NOT NULL,
    file_size bigint DEFAULT 0 NOT NULL,
    file_sha256 character varying(64) NOT NULL,
    page_count integer,
    source_object_key character varying(500),
    source_file_name character varying(255),
    source_mime character varying(160),
    source_size bigint,
    extracted_units jsonb,
    extracted_at timestamp with time zone,
    uploaded_by uuid,
    approved_by uuid,
    approved_at timestamp with time zone,
    rejected_reason text,
    went_live_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    duration_seconds double precision,
    file_bucket character varying(20) DEFAULT 'documents'::character varying NOT NULL,
    CONSTRAINT ck_library_document_versions_bucket CHECK (((file_bucket)::text = ANY (ARRAY[('documents'::character varying)::text, ('assets'::character varying)::text]))),
    CONSTRAINT ck_library_document_versions_note CHECK ((btrim(change_note) <> ''::text)),
    CONSTRAINT ck_library_document_versions_status CHECK (((status)::text = ANY (ARRAY[('draft'::character varying)::text, ('pending_approval'::character varying)::text, ('live'::character varying)::text, ('superseded'::character varying)::text, ('rejected'::character varying)::text])))
);

CREATE TABLE public.library_documents (
    id uuid NOT NULL,
    title character varying(300) NOT NULL,
    description text,
    category_id uuid NOT NULL,
    audiences text[] DEFAULT '{internal}'::text[] NOT NULL,
    owner_id uuid,
    language character varying(8) DEFAULT 'en'::character varying NOT NULL,
    language_group_id uuid,
    review_by date,
    last_reminded_at timestamp with time zone,
    current_version_id uuid,
    kind character varying(20) DEFAULT 'uploaded'::character varying NOT NULL,
    is_deleted boolean DEFAULT false NOT NULL,
    created_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ck_library_documents_audiences CHECK (((cardinality(audiences) >= 1) AND (audiences <@ ARRAY['internal'::text, 'customer'::text, 'partner'::text]))),
    CONSTRAINT ck_library_documents_kind CHECK (((kind)::text = ANY (ARRAY[('uploaded'::character varying)::text, ('price_list'::character varying)::text])))
);

CREATE TABLE public.library_downloads (
    id uuid NOT NULL,
    fingerprint character varying(20) NOT NULL,
    version_id uuid NOT NULL,
    document_id uuid NOT NULL,
    kind character varying(20) NOT NULL,
    user_id uuid,
    partner_account_id uuid,
    recipient_name character varying(255),
    recipient_company character varying(255),
    channel character varying(20),
    file_sha256 character varying(64),
    ip character varying(64),
    user_agent character varying(500),
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ck_library_downloads_kind CHECK (((kind)::text = ANY (ARRAY[('staff_download'::character varying)::text, ('customer_send'::character varying)::text, ('partner_download'::character varying)::text, ('nary_send'::character varying)::text, ('source_download'::character varying)::text])))
);

CREATE TABLE public.mailing_list_contacts (
    list_id uuid NOT NULL,
    email character varying(255) NOT NULL,
    first_name character varying(100),
    last_name character varying(100),
    company character varying(255),
    is_subscribed boolean NOT NULL,
    unsubscribed_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.mailing_lists (
    name character varying(255) NOT NULL,
    description text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.maintenance_equipment (
    name character varying(255) NOT NULL,
    code character varying(50) NOT NULL,
    equipment_type character varying(100),
    brand character varying(100),
    model character varying(100),
    serial_number character varying(100),
    location character varying(200),
    purchase_date date,
    status character varying(30),
    asset_id uuid,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.maintenance_plans (
    equipment_id uuid NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    frequency_type character varying(20) NOT NULL,
    frequency_value integer NOT NULL,
    last_performed_at timestamp with time zone,
    next_due_at timestamp with time zone,
    is_active boolean NOT NULL,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.maintenance_requests (
    number character varying(50) NOT NULL,
    equipment_id uuid NOT NULL,
    reported_by_id uuid,
    description text NOT NULL,
    priority character varying(20),
    status character varying(30),
    submitted_at timestamp with time zone,
    reviewed_by_id uuid,
    reviewed_at timestamp with time zone,
    rejection_reason text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.maintenance_work_orders (
    number character varying(50) NOT NULL,
    request_id uuid,
    equipment_id uuid NOT NULL,
    work_type character varying(20),
    description text NOT NULL,
    status character varying(30),
    assigned_to_id uuid,
    scheduled_date date,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    downtime_hours numeric(6,2),
    cost numeric(14,2),
    currency character varying(3),
    vendor_id uuid,
    notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.marketing_campaigns (
    name character varying(255) NOT NULL,
    channel character varying(50),
    objective character varying(100),
    status character varying(30),
    budget numeric(18,2),
    actual_spend numeric(18,2),
    currency character varying(3),
    start_date date,
    end_date date,
    target_audience text,
    description text,
    goals text,
    utm_source character varying(100),
    utm_medium character varying(100),
    utm_campaign character varying(100),
    leads_generated integer,
    deals_influenced integer,
    revenue_influenced numeric(18,2),
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.mcp_oauth_clients (
    client_id character varying(64) NOT NULL,
    client_name character varying(200) NOT NULL,
    redirect_uris jsonb NOT NULL,
    last_used_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.mcp_oauth_codes (
    code_hash character varying(64) NOT NULL,
    client_id character varying(64) NOT NULL,
    user_id uuid NOT NULL,
    redirect_uri text NOT NULL,
    code_challenge character varying(128) NOT NULL,
    scopes jsonb NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    used_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.mdf_claims (
    mdf_request_id uuid NOT NULL,
    submitted_by_id uuid,
    claimed_amount numeric(18,2) NOT NULL,
    proof_file_url character varying(500),
    description text,
    status character varying(20),
    reviewed_by_id uuid,
    reviewed_at timestamp with time zone,
    paid_at timestamp with time zone,
    payment_reference character varying(255),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.mdf_requests (
    partner_id uuid NOT NULL,
    submitted_by_id uuid,
    title character varying(255) NOT NULL,
    description text,
    activity_type character varying(50) NOT NULL,
    requested_amount numeric(18,2) NOT NULL,
    approved_amount numeric(18,2),
    currency character varying(3),
    planned_date date,
    planned_location character varying(255),
    expected_outcome text,
    status character varying(30),
    reviewed_by_id uuid,
    reviewed_at timestamp with time zone,
    rejection_reason text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.memos (
    memo_number character varying(30) NOT NULL,
    subject character varying(500) NOT NULL,
    memo_type character varying(30) NOT NULL,
    body text NOT NULL,
    sender_id uuid NOT NULL,
    sender_name character varying(255) NOT NULL,
    sender_position character varying(255),
    sender_department character varying(255),
    sender_signed_at timestamp with time zone NOT NULL,
    recipient_id uuid,
    channel_id uuid,
    status character varying(30) NOT NULL,
    recalled_at timestamp with time zone,
    acknowledgements jsonb,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.message_reactions (
    message_id character varying(36) NOT NULL,
    message_source character varying(10) NOT NULL,
    user_id uuid NOT NULL,
    emoji character varying(10) NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.nary_autonomy (
    action character varying(60) NOT NULL,
    streak integer DEFAULT 0 NOT NULL,
    candidate_since timestamp with time zone,
    approvals_total integer DEFAULT 0 NOT NULL,
    edits_total integer DEFAULT 0 NOT NULL,
    rejects_total integer DEFAULT 0 NOT NULL,
    last_changed_at timestamp with time zone,
    last_change character varying(120),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.nary_cadences (
    quotation_id uuid NOT NULL,
    deal_id uuid,
    contact_id uuid,
    day integer NOT NULL,
    channel character varying(10) NOT NULL,
    intent text,
    topic character varying(30),
    due_at timestamp with time zone NOT NULL,
    status character varying(20) DEFAULT 'scheduled'::character varying NOT NULL,
    reason text,
    card_id uuid,
    done_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    draft_text text,
    merged_into uuid
);

CREATE TABLE public.nary_owner_topics (
    kind character varying(30) NOT NULL,
    tag character varying(200) NOT NULL,
    text text NOT NULL,
    ref_type character varying(40),
    ref_id uuid,
    facts jsonb,
    status character varying(20) DEFAULT 'queued'::character varying NOT NULL,
    urgent boolean DEFAULT false NOT NULL,
    anytime boolean DEFAULT false NOT NULL,
    numbered jsonb,
    resume jsonb,
    send_after timestamp with time zone,
    sent_at timestamp with time zone,
    wa_message_keys jsonb,
    conversation_id uuid,
    remind_at timestamp with time zone,
    reminded_at timestamp with time zone,
    answered_at timestamp with time zone,
    answer text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.nary_chat_pauses (
    chat_id character varying(120) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    paused_until timestamp with time zone NOT NULL,
    reason text NOT NULL,
    message_key character varying(128)
);

CREATE TABLE public.nary_cost_sheets (
    document_id uuid,
    deal_id uuid,
    content jsonb DEFAULT '{}'::jsonb NOT NULL,
    currency character varying(3) DEFAULT 'USD'::character varying NOT NULL,
    status character varying(20) DEFAULT 'draft'::character varying NOT NULL,
    model character varying(80),
    run_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.nary_event_followups (
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    event_id uuid NOT NULL,
    kind character varying(40) NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    done_at timestamp with time zone
);

CREATE TABLE public.nary_expected_arrivals (
    lead_id uuid,
    phone character varying(32),
    email character varying(255),
    name character varying(200),
    company character varying(255),
    message text,
    due_at timestamp with time zone NOT NULL,
    status character varying(20) DEFAULT 'pending'::character varying NOT NULL,
    chat_id character varying(120),
    arrived_at timestamp with time zone,
    notified_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.nary_file_links (
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    file_id uuid NOT NULL,
    source character varying(20) NOT NULL,
    source_ref character varying(300) NOT NULL,
    filename character varying(255) NOT NULL,
    received_at timestamp with time zone,
    wa_chat_id character varying(120),
    wa_message_key character varying(128),
    crm_thread_id uuid,
    crm_message_id uuid,
    crm_attachment_id uuid,
    email_message_id uuid,
    email_attachment_id character varying(64),
    mailbox_owner_id uuid,
    attachment_id uuid,
    chat_user_id uuid,
    contact_id uuid,
    organization_id uuid,
    deal_id uuid,
    lead_id uuid,
    added_by_id uuid
);

CREATE TABLE public.nary_files (
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    sha256 character varying(64) NOT NULL,
    filename character varying(255) NOT NULL,
    mime character varying(120),
    size_bytes bigint DEFAULT 0 NOT NULL,
    object_key character varying(400) NOT NULL,
    kind character varying(20) NOT NULL,
    status character varying(20) DEFAULT 'stored'::character varying NOT NULL,
    page_count integer,
    units jsonb,
    text text,
    error text,
    vision_pages integer DEFAULT 0 NOT NULL,
    extract_started_at timestamp with time zone,
    extracted_at timestamp with time zone
);

CREATE TABLE public.nary_identity_links (
    subject_type character varying(20) NOT NULL,
    subject_ref character varying(120) NOT NULL,
    contact_id uuid,
    lead_id uuid,
    deal_id uuid,
    tier character varying(10) NOT NULL,
    score integer DEFAULT 0 NOT NULL,
    signals jsonb DEFAULT '[]'::jsonb NOT NULL,
    status character varying(20) DEFAULT 'linked'::character varying NOT NULL,
    cascaded boolean DEFAULT false NOT NULL,
    review text,
    previous jsonb,
    decided_by_id uuid,
    decided_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.nary_insights (
    kind character varying(20) NOT NULL,
    deal_id uuid,
    lead_id uuid,
    meeting_id uuid,
    content jsonb DEFAULT '{}'::jsonb NOT NULL,
    status character varying(20) DEFAULT 'draft'::character varying NOT NULL,
    model character varying(80),
    run_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.nary_meeting_files (
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    kind character varying(20) NOT NULL,
    meeting_id uuid,
    deal_id uuid,
    filename character varying(255) NOT NULL,
    mime character varying(120),
    size_bytes bigint DEFAULT 0 NOT NULL,
    object_key character varying(400) NOT NULL,
    source character varying(20) NOT NULL,
    wa_message_key character varying(128),
    status character varying(20) NOT NULL,
    progress integer DEFAULT 0 NOT NULL,
    error text,
    stt_job_id character varying(64),
    submits integer DEFAULT 0 NOT NULL,
    language character varying(10),
    duration_seconds double precision,
    transcript text,
    result jsonb,
    requested_by_id uuid,
    next_poll_at timestamp with time zone,
    started_at timestamp with time zone,
    finished_at timestamp with time zone
);

CREATE TABLE public.nary_meetings (
    deal_id uuid,
    lead_id uuid,
    contact_id uuid,
    chat_id character varying(120),
    channel character varying(10) DEFAULT 'whatsapp'::character varying NOT NULL,
    language character varying(5),
    customer_tz character varying(60),
    country character varying(2),
    slots jsonb DEFAULT '[]'::jsonb NOT NULL,
    status character varying(20) DEFAULT 'proposed'::character varying NOT NULL,
    picked integer,
    starts_at timestamp with time zone,
    ends_at timestamp with time zone,
    event_id character varying(255),
    meet_link character varying(500),
    attendee_email character varying(255),
    title character varying(255),
    day_reminder_at timestamp with time zone,
    hour_reminder_at timestamp with time zone,
    brief_at timestamp with time zone,
    card_id uuid,
    notes text,
    origin character varying(10) DEFAULT 'nary'::character varying NOT NULL,
    booked_by_id uuid,
    attendees jsonb DEFAULT '[]'::jsonb NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.nary_memory (
    layer character varying(20) NOT NULL,
    subject_type character varying(30),
    subject_id character varying(64),
    title character varying(200),
    content text NOT NULL,
    source_type character varying(30) NOT NULL,
    source_ref character varying(200),
    author_id uuid,
    authored_by_nary boolean DEFAULT false NOT NULL,
    trusted boolean DEFAULT true NOT NULL,
    status character varying(20) DEFAULT 'approved'::character varying NOT NULL,
    approved_by_id uuid,
    approved_at timestamp with time zone,
    expires_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.nary_outbound_log (
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    channel character varying(10) NOT NULL,
    chat_id character varying(120),
    email character varying(255),
    mailbox_id uuid,
    contact_id uuid,
    deal_id uuid,
    card_id uuid,
    action character varying(60),
    trigger_key character varying(160),
    automatic boolean DEFAULT false NOT NULL,
    kind character varying(30),
    message_ref character varying(255)
);

CREATE TABLE public.nary_rate_card (
    role character varying(80) NOT NULL,
    seniority character varying(10) NOT NULL,
    cost_base numeric(18,2) NOT NULL,
    source text,
    valid_from date NOT NULL,
    status character varying(20) DEFAULT 'active'::character varying NOT NULL,
    proposed_by_nary boolean DEFAULT false NOT NULL,
    approved_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.nary_reply_alerts (
    channel character varying(10) NOT NULL,
    chat_id character varying(120),
    thread_id uuid,
    message_ref character varying(255),
    contact_id uuid,
    lead_id uuid,
    deal_id uuid,
    who character varying(255),
    summary text NOT NULL,
    needs_action boolean DEFAULT false NOT NULL,
    action text,
    card_id uuid,
    topic_id uuid,
    briefed_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.nary_reply_jobs (
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    chat_id character varying(120) NOT NULL,
    message_key character varying(128) NOT NULL,
    first_message_key character varying(128),
    messages integer DEFAULT 1 NOT NULL,
    origin character varying(20),
    status character varying(20) DEFAULT 'pending'::character varying NOT NULL,
    due_at timestamp with time zone DEFAULT now() NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    reason text,
    card_id uuid,
    run_id uuid,
    started_at timestamp with time zone,
    finished_at timestamp with time zone
);

CREATE TABLE public.nary_role_assignments (
    sop_role character varying(40) NOT NULL,
    is_nary boolean DEFAULT false NOT NULL,
    user_id uuid,
    note character varying(255),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.nary_scores (
    lead_id uuid,
    deal_id uuid,
    chat_id character varying(120),
    budget integer DEFAULT 0 NOT NULL,
    authority integer DEFAULT 0 NOT NULL,
    need integer DEFAULT 0 NOT NULL,
    timeline integer DEFAULT 0 NOT NULL,
    total integer DEFAULT 0 NOT NULL,
    class character varying(10) NOT NULL,
    red_flags jsonb DEFAULT '[]'::jsonb NOT NULL,
    green_flags jsonb DEFAULT '[]'::jsonb NOT NULL,
    rationale text,
    intent character varying(20),
    product character varying(40),
    model character varying(80),
    cascaded boolean DEFAULT false NOT NULL,
    run_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.nary_triage_verdicts (
    subject_type character varying(20) NOT NULL,
    subject_ref character varying(120) NOT NULL,
    lead_id uuid,
    verdict character varying(20) NOT NULL,
    language character varying(5),
    reason text,
    excerpt text,
    job character varying(60),
    model character varying(80),
    cascaded boolean DEFAULT false NOT NULL,
    cascade_reason character varying(200),
    corrected_to character varying(20),
    corrected_by_id uuid,
    corrected_at timestamp with time zone,
    correction_note text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.note_templates (
    name character varying(200) NOT NULL,
    body text NOT NULL,
    doc_type character varying(30),
    sort_order integer,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.notifications (
    user_id uuid NOT NULL,
    type character varying(100) NOT NULL,
    title character varying(255) NOT NULL,
    body text,
    link character varying(500),
    entity_type character varying(100),
    entity_id character varying(100),
    is_read boolean,
    read_at character varying(50),
    priority character varying(20),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.offer_letters (
    number character varying(50) NOT NULL,
    applicant_id uuid NOT NULL,
    job_opening_id uuid,
    hiring_request_id uuid,
    position_title character varying(255) NOT NULL,
    department character varying(100),
    reports_to_employee_id uuid,
    employment_type character varying(30),
    salary_gross_monthly numeric(18,2) NOT NULL,
    salary_currency character varying(3),
    start_date date,
    probation_months integer,
    benefits text,
    other_terms text,
    acceptance_deadline date,
    status character varying(30),
    sent_at timestamp with time zone,
    responded_at timestamp with time zone,
    candidate_response_notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.office_locations (
    name character varying(100) NOT NULL,
    address text,
    latitude numeric(10,7) NOT NULL,
    longitude numeric(10,7) NOT NULL,
    radius_meters integer,
    is_active boolean,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.organizations (
    name character varying(255) NOT NULL,
    type character varying[],
    industry character varying(100),
    country character varying(100),
    city character varying(100),
    address text,
    postal_code character varying(20),
    website character varying(255),
    phone character varying(50),
    email character varying(255),
    tax_id character varying(100),
    registration_number character varying(100),
    annual_revenue numeric(18,2),
    revenue_currency character varying(3),
    employee_count integer,
    description text,
    notes text,
    tags character varying[],
    assigned_to_id uuid,
    created_by_id uuid,
    custom jsonb DEFAULT '{}'::jsonb NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone,
    quote_without_tax boolean DEFAULT false NOT NULL
);

CREATE TABLE public.partner_accounts (
    organization_id uuid,
    primary_contact_id uuid,
    company_name character varying(255),
    contact_name character varying(255),
    contact_email character varying(255),
    contact_phone character varying(50),
    country character varying(100),
    website character varying(500),
    notes text,
    agreement_ref character varying(50),
    agreement_url character varying(500),
    partner_type character varying(50) NOT NULL,
    tier character varying(30),
    status character varying(30),
    territory_countries character varying[],
    territory_verticals character varying[],
    authorized_products character varying[],
    authorized_services character varying[],
    is_exclusive boolean,
    exclusivity_scope jsonb,
    annual_min_units_y1 integer,
    annual_min_revenue_y1 numeric(18,2),
    annual_min_units_y2 integer,
    annual_min_revenue_y2 numeric(18,2),
    annual_min_units_y3 integer,
    annual_min_revenue_y3 numeric(18,2),
    commitment_currency character varying(3),
    certified_staff_count integer,
    certification_details jsonb,
    cumulative_units integer,
    cumulative_revenue numeric(18,2),
    current_year_units integer,
    current_year_revenue numeric(18,2),
    tier_reviewed_at date,
    agreement_signed_at timestamp with time zone,
    agreement_expires_at date,
    joined_at date,
    assigned_am_id uuid,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.partner_certifications (
    partner_user_id uuid NOT NULL,
    module_id uuid NOT NULL,
    issued_at timestamp with time zone,
    expires_at timestamp with time zone,
    certificate_url character varying(500),
    badge_slug character varying(100),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.partner_commissions (
    partner_id uuid NOT NULL,
    deal_registration_id uuid,
    invoice_id uuid,
    commission_type character varying(50) NOT NULL,
    base_amount numeric(18,2) NOT NULL,
    rate numeric(6,4) NOT NULL,
    amount numeric(18,2) NOT NULL,
    currency character varying(3),
    period_month integer,
    period_year integer,
    status character varying(30),
    approved_by_id uuid,
    paid_at timestamp with time zone,
    payment_reference character varying(255),
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.partner_deal_registrations (
    registration_number character varying(50),
    partner_id uuid NOT NULL,
    customer_org_id uuid,
    customer_org_name character varying(255),
    customer_contact_name character varying(255),
    customer_contact_email character varying(255),
    customer_country character varying(100),
    title character varying(255) NOT NULL,
    scope text,
    estimated_value numeric(18,2),
    currency character varying(3),
    expected_close_date date,
    engagement_role character varying(255),
    products_services character varying[],
    status character varying(30),
    registered_at timestamp with time zone NOT NULL,
    protection_days integer,
    expires_at date,
    extended_until date,
    approved_at timestamp with time zone,
    approved_by_id uuid,
    rejection_reason character varying(500),
    deal_id uuid,
    revenue_credited numeric(18,2),
    revenue_credited_currency character varying(3),
    revenue_credited_year integer,
    status_before_close character varying(30),
    conflicting_registration_id uuid,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.partner_leads (
    partner_id uuid NOT NULL,
    assigned_by_id uuid,
    customer_name character varying(255) NOT NULL,
    customer_email character varying(255),
    customer_company character varying(255),
    customer_country character varying(100),
    product_interest character varying[],
    notes text,
    status character varying(30),
    converted_deal_id uuid,
    assigned_at timestamp with time zone,
    expires_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.partner_module_progress (
    partner_user_id uuid NOT NULL,
    module_id uuid NOT NULL,
    status character varying(20),
    current_chapter integer,
    score integer,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    attempts integer DEFAULT 0 NOT NULL
);

CREATE TABLE public.partner_resources (
    title character varying(255) NOT NULL,
    description text,
    resource_type character varying(50),
    file_url character varying(500),
    thumbnail_url character varying(500),
    version character varying(20),
    min_tier character varying(30),
    partner_types character varying[],
    is_active boolean,
    download_count integer,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.payment_schedules (
    quotation_id uuid,
    invoice_id uuid,
    project_id uuid,
    milestone_id uuid,
    name character varying(255) NOT NULL,
    percentage numeric(5,2),
    amount numeric(18,2),
    currency character varying(3),
    sequence integer NOT NULL,
    due_trigger character varying(50),
    due_date date,
    status character varying(30),
    proforma_invoice_id uuid,
    paid_at date,
    notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.payments (
    invoice_id uuid NOT NULL,
    amount numeric(18,2) NOT NULL,
    currency character varying(3),
    exchange_rate numeric(18,6),
    payment_method character varying(100),
    payment_reference character varying(255),
    bank_name character varying(100),
    payment_date date NOT NULL,
    received_at timestamp with time zone,
    status character varying(30),
    confirmed_by_id uuid,
    proof_url character varying(500),
    bank_account_id uuid,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.payroll_lines (
    payroll_run_id uuid NOT NULL,
    employee_id uuid NOT NULL,
    gross_salary numeric(18,2) NOT NULL,
    allowances numeric(18,2),
    overtime_pay numeric(18,2),
    deductions numeric(18,2),
    income_tax numeric(18,2),
    taxable_gross numeric(18,2),
    net_salary numeric(18,2) NOT NULL,
    days_worked integer,
    days_absent integer,
    notes text,
    payment_reference character varying(100),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    contributions jsonb DEFAULT '{}'::jsonb NOT NULL,
    employee_contributions numeric(18,2),
    employer_contributions numeric(18,2)
);

CREATE TABLE public.payroll_runs (
    month integer NOT NULL,
    year integer NOT NULL,
    status character varying(30),
    total_gross numeric(18,2),
    total_income_tax numeric(18,2),
    total_contributions numeric(18,2),
    total_net numeric(18,2),
    currency character varying(3),
    notes text,
    calculated_by_id uuid,
    approved_by_id uuid,
    approved_at timestamp with time zone,
    paid_at timestamp with time zone,
    bank_account_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.permissions (
    code character varying(100) NOT NULL,
    name character varying(200) NOT NULL,
    module character varying(50) NOT NULL,
    description text,
    is_system_permission boolean,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.personal_access_tokens (
    user_id uuid NOT NULL,
    name character varying(120) NOT NULL,
    token_prefix character varying(16) NOT NULL,
    token_hash character varying(255) NOT NULL,
    scopes jsonb,
    last_used_at timestamp with time zone,
    expires_at timestamp with time zone,
    revoked_at timestamp with time zone,
    kind character varying(10) DEFAULT 'pat'::character varying NOT NULL,
    client_id character varying(64),
    refresh_token_hash character varying(64),
    refresh_expires_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.portal_registration_requests (
    portal_type character varying(20) NOT NULL,
    status character varying(20) NOT NULL,
    company_name character varying(255) NOT NULL,
    contact_name character varying(255) NOT NULL,
    email character varying(255) NOT NULL,
    phone character varying(50),
    vendor_type character varying(50),
    partner_type character varying(50),
    country character varying(100),
    website character varying(500),
    notes text,
    reviewed_by_id uuid,
    reviewed_at timestamp with time zone,
    rejection_reason text,
    created_user_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.product_catalog (
    sku character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    short_name character varying(100),
    description text,
    category character varying(100),
    family character varying(50),
    product_type character varying(30) NOT NULL,
    unit character varying(30),
    sale_model character varying(20) NOT NULL,
    price_metric character varying(20) NOT NULL,
    rate_period_months integer,
    billing_interval_months integer,
    min_quantity numeric(10,2),
    min_term_months integer,
    unit_equivalents numeric(6,2),
    modules character varying[],
    included_support_months integer,
    pricing_mode character varying(20) NOT NULL,
    percent_rate numeric(6,3),
    addon_for character varying[],
    whitelabel_free_min_qty integer,
    whitelabel_rule jsonb,
    is_whitelabel boolean NOT NULL,
    terms_note text,
    cost_price numeric(18,2),
    is_active boolean,
    is_orderable boolean,
    image_url character varying(500),
    hardware_product_id uuid,
    product_line_id uuid,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone,
    segment character varying(30),
    box_licence_value numeric(18,2),
    price_status character varying(10) DEFAULT 'locked'::character varying NOT NULL,
    CONSTRAINT ck_product_catalog_price_status CHECK (((price_status)::text = ANY (ARRAY[('locked'::character varying)::text, ('proposed'::character varying)::text])))
);

CREATE TABLE public.product_lines (
    code character varying(64) NOT NULL,
    name character varying(200) NOT NULL,
    category character varying(30),
    color character varying(16),
    description text,
    is_active boolean NOT NULL,
    sort_order integer,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.production_orders (
    number character varying(50) NOT NULL,
    product_id uuid NOT NULL,
    bom_id uuid,
    sales_order_ref character varying(100),
    invoice_id uuid,
    customer_org_id uuid,
    quantity integer NOT NULL,
    status character varying(30),
    components_reserved boolean DEFAULT false NOT NULL,
    planned_start date,
    planned_end date,
    actual_start date,
    actual_end date,
    assigned_to_id uuid,
    notes text,
    created_by_id uuid,
    total_bom_cost numeric(18,2),
    actual_material_cost numeric(18,2),
    cost_currency character varying(3),
    is_subcontracted boolean,
    subcontractor_vendor_id uuid,
    subcontractor_work_order_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.production_units (
    production_order_id uuid NOT NULL,
    product_id uuid NOT NULL,
    bom_id uuid,
    serial_number character varying(100) NOT NULL,
    hardware_revision character varying(20),
    status character varying(30),
    produced_at timestamp with time zone,
    flashed_at timestamp with time zone,
    firmware_version character varying(50),
    software_version character varying(50),
    invoice_id uuid,
    shipped_at timestamp with time zone,
    tracking_number character varying(100),
    delivery_date date,
    warranty_expiry date,
    notes text,
    produced_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    mac character varying(64),
    sealed_at timestamp with time zone,
    is_test boolean DEFAULT false NOT NULL
);

CREATE TABLE public.project_members (
    project_id uuid NOT NULL,
    user_id uuid NOT NULL,
    role character varying(50),
    allocation_percentage integer,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.project_milestones (
    project_id uuid NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    due_date date NOT NULL,
    status character varying(30),
    triggers_invoice boolean,
    invoice_percentage numeric(5,2),
    invoice_id uuid,
    invoice_triggered_at timestamp with time zone,
    achieved_at timestamp with time zone,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.project_phases (
    project_id uuid NOT NULL,
    name character varying(255) NOT NULL,
    sort_order integer,
    status character varying(30),
    start_date date,
    end_date date,
    description text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.project_tasks (
    project_id uuid NOT NULL,
    phase_id uuid,
    parent_task_id uuid,
    title character varying(500) NOT NULL,
    description text,
    status character varying(30),
    priority character varying(20),
    assigned_to_id uuid,
    created_by_id uuid,
    due_date date,
    estimated_hours numeric(6,2),
    actual_hours numeric(6,2),
    sort_order integer,
    tags character varying[],
    epic_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.projects (
    name character varying(255) NOT NULL,
    project_number character varying(50),
    project_type character varying(50) NOT NULL,
    deal_id uuid,
    contract_id uuid,
    customer_org_id uuid,
    customer_contact_id uuid,
    status character varying(30),
    pm_id uuid,
    tech_lead_id uuid,
    created_by_id uuid,
    budget numeric(18,2),
    currency character varying(3),
    cost_to_date numeric(18,2),
    start_date date,
    end_date date,
    actual_start_date date,
    actual_end_date date,
    description text,
    objectives text,
    deliverables jsonb,
    risks jsonb,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.purchase_order_items (
    po_id uuid NOT NULL,
    component_sku character varying(50),
    description character varying(500) NOT NULL,
    quantity_ordered numeric(10,3) NOT NULL,
    quantity_received numeric(10,3),
    quantity_invoiced numeric(10,3),
    unit character varying(20),
    unit_price numeric(18,4) NOT NULL,
    currency character varying(3),
    amount numeric(18,2) NOT NULL,
    rfq_line_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.purchase_orders (
    number character varying(50) NOT NULL,
    vendor_id uuid NOT NULL,
    contractor_request_id uuid,
    status character varying(30),
    currency character varying(3),
    subtotal numeric(18,2),
    tax_amount numeric(18,2),
    shipping_cost numeric(18,2),
    total numeric(18,2),
    order_date date,
    expected_delivery date,
    actual_delivery date,
    shipping_address text,
    incoterms character varying(20),
    rfq_id uuid,
    approval_status character varying(20),
    requested_by_id uuid NOT NULL,
    approved_by_id uuid,
    approved_at timestamp with time zone,
    notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.purchase_rfq_line_responses (
    rfq_id uuid NOT NULL,
    rfq_line_id uuid NOT NULL,
    vendor_id uuid NOT NULL,
    unit_price numeric(18,4) NOT NULL,
    currency character varying(3),
    lead_time_days numeric(5,0),
    notes text,
    is_selected boolean,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.purchase_rfq_lines (
    rfq_id uuid NOT NULL,
    sort_order numeric(5,0),
    component_sku character varying(50),
    description character varying(500) NOT NULL,
    quantity numeric(10,3) NOT NULL,
    unit character varying(20),
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.purchase_rfq_vendor_invites (
    rfq_id uuid NOT NULL,
    vendor_id uuid NOT NULL,
    status character varying(30),
    invited_at timestamp with time zone,
    responded_at timestamp with time zone,
    response_notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.purchase_rfqs (
    number character varying(50) NOT NULL,
    title character varying(255) NOT NULL,
    description text,
    status character varying(30),
    deadline date,
    currency character varying(3),
    notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.qc_records (
    unit_id uuid NOT NULL,
    checked_by_id uuid NOT NULL,
    checked_at timestamp with time zone NOT NULL,
    result character varying(20) NOT NULL,
    checklist_items jsonb,
    defects_found jsonb,
    corrective_actions text,
    recheck_required boolean,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.quotation_items (
    quotation_id uuid NOT NULL,
    product_id uuid,
    sort_order integer,
    sku character varying(50),
    name character varying(255) NOT NULL,
    description text,
    quantity numeric(10,2) NOT NULL,
    unit character varying(30),
    unit_price numeric(18,2) NOT NULL,
    currency character varying(3),
    discount_percentage numeric(5,2),
    discount_type character varying(20),
    amount numeric(18,2) NOT NULL,
    image_url character varying(500),
    is_optional boolean,
    parent_item_id uuid,
    sale_model character varying(20),
    price_metric character varying(20),
    rate_period_months integer,
    billing_interval_months integer,
    term_months integer,
    list_unit_price numeric(18,2),
    price_rule character varying(30),
    tax_id uuid,
    tax_code character varying(50),
    tax_pct numeric(8,4) NOT NULL,
    tax_amount numeric(18,2) NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.quotations (
    number character varying(50) NOT NULL,
    revision integer,
    full_number character varying(80),
    revision_group character varying(50),
    previous_version_id uuid,
    revision_reason text,
    subject character varying(500),
    deal_id uuid,
    customer_org_id uuid,
    customer_contact_id uuid,
    status character varying(30),
    currency character varying(3),
    exchange_rate numeric(24,12),
    subtotal numeric(18,2),
    discount_type character varying(20),
    discount_value numeric(18,2),
    discount_amount numeric(18,2),
    tax_amount numeric(18,2),
    tax_lines jsonb,
    total numeric(18,2),
    rounded_total numeric(18,2),
    total_quantity integer,
    issue_date date NOT NULL,
    valid_until date NOT NULL,
    validity_days integer,
    payment_terms jsonb,
    payment_terms_days integer,
    procurement_type character varying(20),
    shipping_terms character varying(50),
    lead_time_days_min integer,
    lead_time_days_max integer,
    customer_po_number character varying(100),
    customer_po_date date,
    customer_po_accepted_verbally boolean,
    notes text,
    internal_notes text,
    pdf_url character varying(500),
    document_id character varying(100),
    signed_at timestamp with time zone,
    signed_by_customer character varying(255),
    signature_data jsonb,
    created_by_id uuid,
    approved_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone,
    discount_note text,
    discount_display character varying(20) DEFAULT 'pdf'::character varying,
    cancelled_at timestamp with time zone,
    cancel_reason text,
    pricing_override_reason text,
    pricing_override_by_id uuid,
    pricing_override_at timestamp with time zone,
    pricing_override_fingerprint character varying(64),
    notes_family character varying(64)
);

CREATE TABLE public.rma_cases (
    number character varying(50) NOT NULL,
    warranty_id uuid NOT NULL,
    unit_id uuid NOT NULL,
    customer_org_id uuid,
    status character varying(30),
    issue_description text NOT NULL,
    reported_by character varying(255),
    reported_at timestamp with time zone,
    diagnosis text,
    resolution character varying(50),
    resolution_notes text,
    replacement_unit_id uuid,
    closed_at timestamp with time zone,
    handled_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.rnd_backlog_items (
    rnd_project_id uuid NOT NULL,
    sprint_id uuid,
    title character varying(500) NOT NULL,
    description text,
    item_type character varying(32),
    status character varying(32),
    priority character varying(16),
    story_points integer,
    assignee_id uuid,
    reporter_id uuid,
    gitlab_issue_id integer,
    gitlab_issue_iid integer,
    gitlab_issue_url character varying(500),
    gitlab_synced_at timestamp with time zone,
    labels jsonb,
    acceptance_criteria text,
    resolved_at timestamp with time zone,
    board_rank integer,
    epic_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone,
    gitlab_repository_id uuid,
    gitlab_snapshot jsonb,
    gitlab_sync_error text,
    gitlab_sync_attempts integer DEFAULT 0 NOT NULL,
    gitlab_retry_at timestamp with time zone
);

CREATE TABLE public.rnd_projects (
    code character varying(32) NOT NULL,
    name character varying(200) NOT NULL,
    description text,
    status character varying(32),
    product_ref character varying(64),
    hardware_product_id uuid,
    lead_engineer_id uuid,
    primary_repo_url character varying(500),
    latest_version character varying(50),
    roadmap_notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.rnd_releases (
    rnd_project_id uuid NOT NULL,
    version character varying(50) NOT NULL,
    release_name character varying(200),
    status character varying(32),
    release_date date,
    release_notes text,
    changelog jsonb,
    gitlab_tag_url character varying(500),
    gitlab_tag_name character varying(100),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.rnd_sprints (
    rnd_project_id uuid NOT NULL,
    name character varying(100) NOT NULL,
    goal text,
    start_date date,
    end_date date,
    status character varying(32),
    velocity_planned integer,
    velocity_achieved integer,
    retrospective_notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.role_permissions (
    role_id uuid NOT NULL,
    permission_id uuid NOT NULL
);

CREATE TABLE public.roles (
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description text,
    is_system_role boolean,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.sales_document_assets (
    document_id uuid,
    kind character varying(20) NOT NULL,
    filename character varying(255),
    mime character varying(80),
    storage_key character varying(400),
    caption character varying(500),
    scene jsonb,
    code text,
    model_name character varying(80),
    status character varying(20) DEFAULT 'ready'::character varying NOT NULL,
    checks jsonb DEFAULT '[]'::jsonb NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    promoted boolean DEFAULT false NOT NULL,
    error text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.sales_document_blocks (
    document_id uuid NOT NULL,
    section character varying(40) NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    block_type character varying(20) NOT NULL,
    data jsonb DEFAULT '{}'::jsonb NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.sales_document_jobs (
    id uuid NOT NULL,
    document_id uuid NOT NULL,
    kind character varying(40) NOT NULL,
    status character varying(20) DEFAULT 'running'::character varying NOT NULL,
    http_status integer,
    result jsonb,
    error text,
    created_by_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    finished_at timestamp with time zone
);

CREATE TABLE public.sales_document_quotations (
    document_id uuid NOT NULL,
    quotation_id uuid NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    label character varying(120),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.sales_documents (
    doc_type character varying(30) DEFAULT 'solution_proposal'::character varying NOT NULL,
    number character varying(60) NOT NULL,
    full_number character varying(80),
    revision integer DEFAULT 1 NOT NULL,
    revision_group character varying(60),
    previous_version_id uuid,
    revision_reason text,
    title character varying(300) NOT NULL,
    subtitle character varying(500),
    language character varying(5) DEFAULT 'en'::character varying NOT NULL,
    deal_id uuid,
    organization_id uuid,
    contact_id uuid,
    status character varying(20) DEFAULT 'draft'::character varying NOT NULL,
    validity_days integer DEFAULT 30 NOT NULL,
    classification character varying(200),
    cover jsonb DEFAULT '{}'::jsonb NOT NULL,
    issue_date date,
    created_by_id uuid,
    sent_at timestamp with time zone,
    origin character varying(20),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.scheduled_messages (
    channel character varying(20) NOT NULL,
    created_by_id uuid NOT NULL,
    contact_id uuid,
    chat_id character varying(255),
    recipient character varying(255) NOT NULL,
    subject character varying(500),
    body text NOT NULL,
    scheduled_at timestamp with time zone NOT NULL,
    sent_at timestamp with time zone,
    status character varying(20),
    retry_count integer,
    last_error text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.sign_audit_logs (
    document_id uuid NOT NULL,
    signer_id uuid,
    action character varying(50) NOT NULL,
    description character varying(500),
    ip_address character varying(45),
    user_agent text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.sign_documents (
    title character varying(255) NOT NULL,
    status character varying(30),
    source_type character varying(30),
    source_id uuid,
    original_file_url character varying(500),
    signed_file_url character varying(500),
    certificate_url character varying(500),
    certificate_generated_at timestamp with time zone,
    sealed_at timestamp with time zone,
    seal_fingerprint character varying(95),
    document_hash character varying(64),
    signature_level character varying(10),
    seal_level character varying(20),
    jurisdictions character varying(50),
    message text,
    expires_at timestamp with time zone,
    completed_at timestamp with time zone,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.sign_fields (
    document_id uuid NOT NULL,
    signer_id uuid,
    field_type character varying(20) NOT NULL,
    page_number integer,
    x numeric(8,2),
    y numeric(8,2),
    width numeric(8,2),
    height numeric(8,2),
    is_required boolean,
    label character varying(255),
    value text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.sign_signers (
    document_id uuid NOT NULL,
    name character varying(255) NOT NULL,
    email character varying(255) NOT NULL,
    sign_order integer NOT NULL,
    role character varying(20),
    token uuid NOT NULL,
    status character varying(20),
    sent_at timestamp with time zone,
    viewed_at timestamp with time zone,
    signed_at timestamp with time zone,
    declined_reason text,
    ip_address character varying(45),
    user_agent text,
    user_id uuid,
    signer_type character varying(20),
    partner_account_id uuid,
    signature_data text,
    initials_data text,
    sign_reason character varying(50),
    consent_version character varying(20),
    consent_agreed_at timestamp with time zone,
    consent_text_sha256 character varying(64),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.staff_whatsapp_links (
    user_id uuid NOT NULL,
    chat_id character varying(120) NOT NULL,
    phone character varying(50),
    label character varying(120),
    is_primary boolean DEFAULT false NOT NULL,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.stock_locations (
    warehouse_id uuid NOT NULL,
    parent_id uuid,
    name character varying(100) NOT NULL,
    code character varying(30) NOT NULL,
    location_type character varying(20),
    is_active boolean NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.stock_moves (
    component_sku character varying(50) NOT NULL,
    component_name character varying(255),
    from_location_id uuid,
    to_location_id uuid,
    quantity numeric(10,3) NOT NULL,
    move_type character varying(20) NOT NULL,
    reference character varying(255),
    moved_by_id uuid,
    moved_at timestamp with time zone NOT NULL,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.system_settings (
    key character varying(200) NOT NULL,
    value text,
    value_type character varying(20),
    category character varying(100) NOT NULL,
    label character varying(200),
    description text,
    is_public boolean,
    is_encrypted boolean,
    updated_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.tax_configs (
    name character varying(100) NOT NULL,
    country_code character varying(3) NOT NULL,
    country_name character varying(100) NOT NULL,
    currency_code character varying(3) NOT NULL,
    currency_symbol character varying(10),
    currency_format character varying(50),
    decimal_separator character varying(1),
    thousands_separator character varying(1),
    fiscal_year_start_month integer,
    date_format character varying(30),
    is_active boolean NOT NULL,
    effective_from date NOT NULL,
    notes text,
    created_by_id uuid,
    updated_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.tax_rules (
    config_id uuid NOT NULL,
    code character varying(50) NOT NULL,
    name character varying(150) NOT NULL,
    description text,
    rate numeric(8,4) NOT NULL,
    is_percentage boolean,
    fixed_amount numeric(18,2),
    is_compound boolean,
    compound_base_codes character varying[],
    tax_type character varying(50) NOT NULL,
    is_withholding boolean,
    withheld_by character varying(20),
    applies_to_sales boolean,
    applies_to_purchases boolean,
    applies_to_services boolean,
    applies_to_products boolean,
    applies_to_imports boolean,
    applies_to_vendor_invoices boolean,
    show_on_quotation boolean,
    show_on_invoice boolean,
    show_on_vendor_invoice boolean,
    minimum_taxable_amount numeric(18,2),
    registration_number_required boolean,
    tax_account_code character varying(50),
    reporting_code character varying(50),
    display_order integer,
    is_active boolean,
    notes text,
    updated_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.taxes (
    code character varying(50) NOT NULL,
    name character varying(200) NOT NULL,
    law_reference character varying(300),
    rate numeric(8,4) NOT NULL,
    rate_base character varying(30),
    tax_type character varying(50) NOT NULL,
    is_active boolean,
    applies_to_sales boolean,
    applies_to_purchases boolean,
    tax_payable_account_id uuid,
    tax_receivable_account_id uuid,
    notes text,
    created_by_id uuid,
    updated_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.termination_requests (
    employee_id uuid NOT NULL,
    initiated_by_id uuid NOT NULL,
    confirmed_by_id uuid,
    confirmed_at timestamp with time zone,
    termination_type character varying(50),
    termination_date date,
    last_working_date date,
    notice_given_date date,
    reason text,
    status character varying(30),
    exit_interview_done boolean,
    assets_returned boolean,
    accounts_deactivated boolean,
    final_payment_processed boolean,
    notes text,
    hr_notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.time_logs (
    task_id uuid,
    project_id uuid NOT NULL,
    user_id uuid NOT NULL,
    date date NOT NULL,
    hours numeric(5,2) NOT NULL,
    description text,
    is_billable boolean,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.training_chapters (
    module_id uuid NOT NULL,
    title character varying(255) NOT NULL,
    content text,
    video_url character varying(500),
    "order" integer,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.training_modules (
    title character varying(255) NOT NULL,
    description text,
    thumbnail_url character varying(500),
    min_tier character varying(30),
    partner_types character varying[],
    is_published boolean,
    pass_score integer,
    certificate_template_url character varying(500),
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.training_quiz_questions (
    chapter_id uuid NOT NULL,
    question text NOT NULL,
    options jsonb NOT NULL,
    correct_index integer NOT NULL,
    "order" integer,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.upload_link_files (
    id uuid NOT NULL,
    link_id uuid NOT NULL,
    file_id uuid NOT NULL,
    file_kind character varying(20) NOT NULL,
    name character varying(255) NOT NULL,
    size bigint NOT NULL,
    mime character varying(120) NOT NULL,
    duration_seconds double precision,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT upload_link_files_file_kind_check CHECK (((file_kind)::text = ANY (ARRAY[('attachment'::character varying)::text, ('nary_file'::character varying)::text])))
);

CREATE TABLE public.upload_links (
    id uuid NOT NULL,
    token_hash character varying(64) NOT NULL,
    target character varying(20) NOT NULL,
    entity_id uuid,
    note text,
    created_by_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    max_files integer NOT NULL,
    files_used integer DEFAULT 0 NOT NULL,
    revoked_at timestamp with time zone,
    CONSTRAINT upload_links_check CHECK ((((target)::text = 'staging'::text) = (entity_id IS NULL))),
    CONSTRAINT upload_links_target_check CHECK (((target)::text = ANY (ARRAY[('lead'::character varying)::text, ('deal'::character varying)::text, ('contact'::character varying)::text, ('organization'::character varying)::text, ('staging'::character varying)::text])))
);

CREATE TABLE public.user_permissions (
    user_id uuid NOT NULL,
    permission_id uuid NOT NULL,
    is_deny boolean
);

CREATE TABLE public.user_roles (
    user_id uuid NOT NULL,
    role_id uuid NOT NULL
);

CREATE TABLE public.user_sessions (
    user_id uuid NOT NULL,
    token_hash character varying(500) NOT NULL,
    token_type character varying(20),
    expires_at timestamp with time zone NOT NULL,
    ip_address character varying(45),
    user_agent text,
    is_revoked boolean,
    revoked_at timestamp with time zone,
    family_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.user_totp (
    user_id uuid NOT NULL,
    secret_encrypted text NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    confirmed_at timestamp with time zone,
    last_used_step bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.user_totp_failures (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    kind character varying(20) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.user_totp_recovery_codes (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    code_hash character varying(100) NOT NULL,
    used_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.users (
    email character varying(255) NOT NULL,
    name character varying(255) NOT NULL,
    password_hash character varying(255) NOT NULL,
    phone character varying(50),
    avatar_url character varying(500),
    signature_data text,
    email_signature_fields jsonb,
    email_signature_enabled boolean,
    wa_signature_name character varying(60),
    wa_signature_custom character varying(300),
    department character varying(100),
    "position" character varying(100),
    is_active boolean NOT NULL,
    is_superadmin boolean NOT NULL,
    is_system_admin boolean NOT NULL,
    portal_type character varying(20) NOT NULL,
    partner_account_id uuid,
    vendor_id uuid,
    last_login_at timestamp with time zone,
    password_changed_at timestamp with time zone DEFAULT now(),
    must_change_password boolean,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    avatar_image bytea,
    avatar_mime character varying(50),
    avatar_hash character varying(64)
);

CREATE TABLE public.vendor_documents (
    vendor_id uuid NOT NULL,
    document_type character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    file_url character varying(500),
    signed_at date,
    expires_at date,
    is_verified boolean,
    verified_by_id uuid,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.vendor_invoices (
    work_order_id uuid,
    purchase_order_id uuid,
    vendor_id uuid NOT NULL,
    vendor_invoice_number character varying(100),
    invoice_date date NOT NULL,
    due_date date,
    amount numeric(18,2) NOT NULL,
    tax_amount numeric(18,2),
    withholding_amount numeric(18,2),
    net_payable numeric(18,2),
    currency character varying(3),
    status character varying(30),
    invoice_file_url character varying(500),
    rejection_reason text,
    reviewed_by_id uuid,
    reviewed_at timestamp with time zone,
    paid_at timestamp with time zone,
    payment_reference character varying(255),
    exchange_rate numeric(24,12),
    bank_account_id uuid,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.vendors (
    organization_id uuid,
    name character varying(255) NOT NULL,
    vendor_type character varying(50) NOT NULL,
    status character varying(30),
    tax_id character varying(50),
    registration_number character varying(100),
    bank_name character varying(100),
    bank_account_name character varying(255),
    bank_account_number character varying(50),
    bank_swift character varying(20),
    bank_country character varying(100),
    withholding_applicable boolean,
    withholding_rate numeric(6,3),
    contact_name character varying(255),
    contact_email character varying(255),
    contact_phone character varying(50),
    contact_whatsapp character varying(50),
    address text,
    country character varying(100),
    has_portal_access boolean,
    rating numeric(3,2),
    total_spend numeric(18,2),
    spend_currency character varying(3),
    onboarded_at date,
    notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.video_rooms (
    room_id character varying(64) NOT NULL,
    name character varying(255),
    created_by_id uuid NOT NULL,
    deal_id uuid,
    project_id uuid,
    is_active boolean,
    max_participants integer,
    started_at timestamp with time zone,
    ended_at timestamp with time zone,
    participants jsonb,
    waiting_participants jsonb,
    guest_token_secret character varying(64),
    invited_user_ids jsonb,
    recording_key character varying(500),
    recording_started_at timestamp with time zone,
    recording_ended_at timestamp with time zone,
    recording_size_bytes integer,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.wa_chats (
    chat_id character varying(120) NOT NULL,
    phone character varying(32),
    display_name character varying(200),
    is_my_contact boolean DEFAULT false NOT NULL,
    status character varying(20) DEFAULT 'active'::character varying NOT NULL,
    customer_started boolean,
    contact_id uuid,
    lead_id uuid,
    deal_id uuid,
    staff_user_id uuid,
    human_until timestamp with time zone,
    checkin_sent_at timestamp with time zone,
    acked_at timestamp with time zone,
    ack_pending_since timestamp with time zone,
    first_inbound_at timestamp with time zone,
    last_inbound_at timestamp with time zone,
    last_outbound_at timestamp with time zone,
    business_profile jsonb,
    profile_fetched_at timestamp with time zone,
    triage character varying(20),
    language character varying(5),
    attribution_asked boolean DEFAULT false NOT NULL,
    excluded_at timestamp with time zone,
    excluded_by_id uuid,
    exclude_reason character varying(40),
    released_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.wa_contact_links (
    phone character varying(50) NOT NULL,
    contact_id uuid NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.wa_hidden_chats (
    chat_id character varying(255) NOT NULL,
    hidden_through integer NOT NULL,
    hidden_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.wa_import_chats (
    chat_id character varying(120) NOT NULL,
    name character varying(200),
    message_count integer,
    last_message_at timestamp with time zone,
    decision character varying(10) DEFAULT 'pending'::character varying NOT NULL,
    decided_by_id uuid,
    decided_at timestamp with time zone,
    import_status character varying(20) DEFAULT 'waiting'::character varying NOT NULL,
    cursor_before character varying(255),
    imported_count integer DEFAULT 0 NOT NULL,
    oldest_at timestamp with time zone,
    error text,
    classification character varying(20),
    batch_id character varying(120),
    read_status character varying(20) DEFAULT 'waiting'::character varying NOT NULL,
    summary text,
    outputs jsonb,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.wa_messages (
    message_key character varying(128) NOT NULL,
    serialized_id character varying(255),
    chat_id character varying(120) NOT NULL,
    direction character varying(10) NOT NULL,
    from_me boolean DEFAULT false NOT NULL,
    sent_by_erp boolean DEFAULT false NOT NULL,
    sent_by_nary boolean DEFAULT false NOT NULL,
    type character varying(30) DEFAULT 'chat'::character varying NOT NULL,
    body text,
    quoted_key character varying(128),
    ack integer,
    wa_ts timestamp with time zone,
    has_media boolean DEFAULT false NOT NULL,
    media_key character varying(300),
    media_mime character varying(120),
    media_name character varying(255),
    media_size integer,
    media_status character varying(20) DEFAULT 'none'::character varying NOT NULL,
    media_description text,
    transcript text,
    location jsonb,
    contact_id uuid,
    lead_id uuid,
    deal_id uuid,
    activity_id uuid,
    origin character varying(10) DEFAULT 'live'::character varying NOT NULL,
    processed_at timestamp with time zone,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.wa_sent_messages (
    message_key character varying(128) NOT NULL,
    chat_id character varying(255),
    user_id uuid,
    sender_name character varying(120) NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    kind character varying(30),
    card_id uuid
);

CREATE TABLE public.warehouses (
    name character varying(100) NOT NULL,
    code character varying(20) NOT NULL,
    address text,
    is_default boolean NOT NULL,
    is_active boolean NOT NULL,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.warranty_records (
    unit_id uuid NOT NULL,
    customer_org_id uuid,
    invoice_id uuid,
    customer_name character varying(255),
    customer_email character varying(255),
    delivery_date date NOT NULL,
    warranty_months integer,
    expiry_date date NOT NULL,
    status character varying(30),
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.whatsapp_sessions (
    phone_number character varying(50) NOT NULL,
    status character varying(30),
    qr_code_data text,
    qr_expires_at character varying(50),
    last_active_at character varying(50),
    session_data json,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.work_order_timesheets (
    work_order_id uuid NOT NULL,
    vendor_user_id uuid,
    period_start date NOT NULL,
    period_end date NOT NULL,
    total_hours numeric(6,2) NOT NULL,
    daily_breakdown jsonb,
    status character varying(30),
    reviewed_by_id uuid,
    reviewed_at timestamp with time zone,
    rejection_reason text,
    notes text,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.work_orders (
    number character varying(50) NOT NULL,
    vendor_id uuid NOT NULL,
    contractor_request_id uuid,
    title character varying(255) NOT NULL,
    description text,
    scope text,
    work_type character varying(50),
    payment_type character varying(20) NOT NULL,
    agreed_rate numeric(18,2),
    rate_unit character varying(20),
    agreed_value numeric(18,2),
    currency character varying(3),
    estimated_hours numeric(8,2),
    status character varying(30),
    start_date date,
    end_date date,
    actual_completion_date date,
    completed_confirmed_by_id uuid,
    completed_confirmed_at timestamp with time zone,
    project_id uuid,
    production_order_id uuid,
    withholding_applicable boolean,
    withholding_rate numeric(6,3),
    withholding_amount numeric(18,2),
    notes text,
    internal_notes text,
    created_by_id uuid,
    id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_deleted boolean NOT NULL,
    deleted_at timestamp with time zone
);

ALTER TABLE ONLY public.accounts
    ADD CONSTRAINT accounts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.activities
    ADD CONSTRAINT activities_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.activities_voided
    ADD CONSTRAINT activities_voided_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.agent_pending_actions
    ADD CONSTRAINT agent_pending_actions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.agent_runs
    ADD CONSTRAINT agent_runs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.agent_steps
    ADD CONSTRAINT agent_steps_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.ai_conversations
    ADD CONSTRAINT ai_conversations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.ai_knowledge_chunks
    ADD CONSTRAINT ai_knowledge_chunks_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.ai_knowledge_docs
    ADD CONSTRAINT ai_knowledge_docs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.ai_messages
    ADD CONSTRAINT ai_messages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.ai_tool_jobs
    ADD CONSTRAINT ai_tool_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.ai_usage_log
    ADD CONSTRAINT ai_usage_log_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.applicants
    ADD CONSTRAINT applicants_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.approval_actions
    ADD CONSTRAINT approval_actions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.approval_policies
    ADD CONSTRAINT approval_policies_entity_type_key UNIQUE (entity_type);

ALTER TABLE ONLY public.approval_policies
    ADD CONSTRAINT approval_policies_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.approval_policy_steps
    ADD CONSTRAINT approval_policy_steps_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.approval_requests
    ADD CONSTRAINT approval_requests_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.asset_categories
    ADD CONSTRAINT asset_categories_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.asset_depreciation_lines
    ADD CONSTRAINT asset_depreciation_lines_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.asset_disposals
    ADD CONSTRAINT asset_disposals_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.attachments
    ADD CONSTRAINT attachments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.attendance_records
    ADD CONSTRAINT attendance_records_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.audit_logs
    ADD CONSTRAINT audit_logs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.bank_accounts
    ADD CONSTRAINT bank_accounts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.bank_statement_lines
    ADD CONSTRAINT bank_statement_lines_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.bank_statements
    ADD CONSTRAINT bank_statements_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.board_columns
    ADD CONSTRAINT board_columns_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.bom_items
    ADD CONSTRAINT bom_items_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.boms
    ADD CONSTRAINT boms_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.budget_lines
    ADD CONSTRAINT budget_lines_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.budgets
    ADD CONSTRAINT budgets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.catalog_prices
    ADD CONSTRAINT catalog_prices_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.catalog_volume_tiers
    ADD CONSTRAINT catalog_volume_tiers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.channel_memberships
    ADD CONSTRAINT channel_memberships_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.channel_messages
    ADD CONSTRAINT channel_messages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.channels
    ADD CONSTRAINT channels_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.clock_events
    ADD CONSTRAINT clock_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.component_inventory
    ADD CONSTRAINT component_inventory_component_sku_key UNIQUE (component_sku);

ALTER TABLE ONLY public.component_inventory
    ADD CONSTRAINT component_inventory_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.contacts
    ADD CONSTRAINT contacts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.content_calendar
    ADD CONSTRAINT content_calendar_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.contractor_requests
    ADD CONSTRAINT contractor_requests_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.contracts
    ADD CONSTRAINT contracts_number_key UNIQUE (number);

ALTER TABLE ONLY public.contracts
    ADD CONSTRAINT contracts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.crm_ai_field_runs
    ADD CONSTRAINT crm_ai_field_runs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.crm_attribute_defs
    ADD CONSTRAINT crm_attribute_defs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.crm_duplicate_candidates
    ADD CONSTRAINT crm_duplicate_candidates_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.crm_email_attachments
    ADD CONSTRAINT crm_email_attachments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.crm_email_messages
    ADD CONSTRAINT crm_email_messages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.crm_email_threads
    ADD CONSTRAINT crm_email_threads_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.crm_embeddings
    ADD CONSTRAINT crm_embeddings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.crm_mailboxes
    ADD CONSTRAINT crm_mailboxes_address_key UNIQUE (address);

ALTER TABLE ONLY public.crm_mailboxes
    ADD CONSTRAINT crm_mailboxes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.crm_mailboxes
    ADD CONSTRAINT crm_mailboxes_prefix_key UNIQUE (prefix);

ALTER TABLE ONLY public.crm_message_copies
    ADD CONSTRAINT crm_message_copies_mailbox_id_folder_imap_uid_key UNIQUE (mailbox_id, folder, imap_uid);

ALTER TABLE ONLY public.crm_message_copies
    ADD CONSTRAINT crm_message_copies_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.crm_reminders
    ADD CONSTRAINT crm_reminders_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.crm_research_people
    ADD CONSTRAINT crm_research_people_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.crm_research_runs
    ADD CONSTRAINT crm_research_runs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.deal_stage_events
    ADD CONSTRAINT deal_stage_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.deals
    ADD CONSTRAINT deals_deal_number_key UNIQUE (deal_number);

ALTER TABLE ONLY public.deals
    ADD CONSTRAINT deals_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.dept_coordinative_links
    ADD CONSTRAINT dept_coordinative_links_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.direct_messages
    ADD CONSTRAINT direct_messages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.discount_allowances
    ADD CONSTRAINT discount_allowances_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.doc_categories
    ADD CONSTRAINT doc_categories_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.document_sequences
    ADD CONSTRAINT document_sequences_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.document_verification_views
    ADD CONSTRAINT document_verification_views_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.document_verifications
    ADD CONSTRAINT document_verifications_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.document_verifications
    ADD CONSTRAINT document_verifications_short_code_key UNIQUE (short_code);

ALTER TABLE ONLY public.document_verifications
    ADD CONSTRAINT document_verifications_token_key UNIQUE (token);

ALTER TABLE ONLY public.domain_events
    ADD CONSTRAINT domain_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.email_accounts
    ADD CONSTRAINT email_accounts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.email_campaigns
    ADD CONSTRAINT email_campaigns_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.email_image_senders
    ADD CONSTRAINT email_image_senders_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.email_messages
    ADD CONSTRAINT email_messages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.email_queue_items
    ADD CONSTRAINT email_queue_items_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.email_queue_items
    ADD CONSTRAINT email_queue_items_tracking_id_key UNIQUE (tracking_id);

ALTER TABLE ONLY public.email_signature_assets
    ADD CONSTRAINT email_signature_assets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.email_suppressions
    ADD CONSTRAINT email_suppressions_email_key UNIQUE (email);

ALTER TABLE ONLY public.email_suppressions
    ADD CONSTRAINT email_suppressions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.employee_allowances
    ADD CONSTRAINT employee_allowances_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.employees
    ADD CONSTRAINT employees_employee_number_key UNIQUE (employee_number);

ALTER TABLE ONLY public.employees
    ADD CONSTRAINT employees_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.epics
    ADD CONSTRAINT epics_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.exchange_rates
    ADD CONSTRAINT exchange_rates_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.expenses
    ADD CONSTRAINT expenses_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.face_check_attempts
    ADD CONSTRAINT face_check_attempts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.face_descriptors
    ADD CONSTRAINT face_descriptors_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.face_enrollments
    ADD CONSTRAINT face_enrollments_employee_id_key UNIQUE (employee_id);

ALTER TABLE ONLY public.face_enrollments
    ADD CONSTRAINT face_enrollments_face_uid_key UNIQUE (face_uid);

ALTER TABLE ONLY public.face_enrollments
    ADD CONSTRAINT face_enrollments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fiscal_periods
    ADD CONSTRAINT fiscal_periods_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fixed_assets
    ADD CONSTRAINT fixed_assets_number_key UNIQUE (number);

ALTER TABLE ONLY public.fixed_assets
    ADD CONSTRAINT fixed_assets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fleet_assignments
    ADD CONSTRAINT fleet_assignments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fleet_fuel_logs
    ADD CONSTRAINT fleet_fuel_logs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fleet_odometer_logs
    ADD CONSTRAINT fleet_odometer_logs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fleet_service_records
    ADD CONSTRAINT fleet_service_records_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fleet_vehicles
    ADD CONSTRAINT fleet_vehicles_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.fleet_vehicles
    ADD CONSTRAINT fleet_vehicles_plate_number_key UNIQUE (plate_number);

ALTER TABLE ONLY public.gitlab_configs
    ADD CONSTRAINT gitlab_configs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.gitlab_issue_queue
    ADD CONSTRAINT gitlab_issue_queue_pkey PRIMARY KEY (repository_id, issue_iid);

ALTER TABLE ONLY public.gitlab_pipeline_runs
    ADD CONSTRAINT gitlab_pipeline_runs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.gitlab_repositories
    ADD CONSTRAINT gitlab_repositories_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.gitlab_user_links
    ADD CONSTRAINT gitlab_user_links_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.gitlab_webhook_events
    ADD CONSTRAINT gitlab_webhook_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.gl_account_roles
    ADD CONSTRAINT gl_account_roles_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.goods_receipts
    ADD CONSTRAINT goods_receipts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.google_accounts
    ADD CONSTRAINT google_accounts_email_key UNIQUE (email);

ALTER TABLE ONLY public.google_accounts
    ADD CONSTRAINT google_accounts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.hardware_products
    ADD CONSTRAINT hardware_products_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.hardware_products
    ADD CONSTRAINT hardware_products_sku_key UNIQUE (sku);

ALTER TABLE ONLY public.helpdesk_categories
    ADD CONSTRAINT helpdesk_categories_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.helpdesk_messages
    ADD CONSTRAINT helpdesk_messages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.helpdesk_sla_policies
    ADD CONSTRAINT helpdesk_sla_policies_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.helpdesk_teams
    ADD CONSTRAINT helpdesk_teams_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.helpdesk_tickets
    ADD CONSTRAINT helpdesk_tickets_number_key UNIQUE (number);

ALTER TABLE ONLY public.helpdesk_tickets
    ADD CONSTRAINT helpdesk_tickets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.helpdesk_tickets
    ADD CONSTRAINT helpdesk_tickets_public_token_key UNIQUE (public_token);

ALTER TABLE ONLY public.hiring_requests
    ADD CONSTRAINT hiring_requests_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.invoice_items
    ADD CONSTRAINT invoice_items_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.invoice_refunds
    ADD CONSTRAINT invoice_refunds_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.invoices
    ADD CONSTRAINT invoices_number_key UNIQUE (number);

ALTER TABLE ONLY public.invoices
    ADD CONSTRAINT invoices_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.issue_activity
    ADD CONSTRAINT issue_activity_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.issue_comments
    ADD CONSTRAINT issue_comments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.job_openings
    ADD CONSTRAINT job_openings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.journal_entries
    ADD CONSTRAINT journal_entries_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.journal_lines
    ADD CONSTRAINT journal_lines_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.kb_article_revisions
    ADD CONSTRAINT kb_article_revisions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.kb_articles
    ADD CONSTRAINT kb_articles_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.kb_articles
    ADD CONSTRAINT kb_articles_slug_key UNIQUE (slug);

ALTER TABLE ONLY public.kb_articles
    ADD CONSTRAINT kb_articles_sop_code_key UNIQUE (sop_code);

ALTER TABLE ONLY public.kb_categories
    ADD CONSTRAINT kb_categories_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.kb_categories
    ADD CONSTRAINT kb_categories_slug_key UNIQUE (slug);

ALTER TABLE ONLY public.kb_embeddings
    ADD CONSTRAINT kb_embeddings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.kiosk_devices
    ADD CONSTRAINT kiosk_devices_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.kiosk_devices
    ADD CONSTRAINT kiosk_devices_token_hash_key UNIQUE (token_hash);

ALTER TABLE ONLY public.lead_attributions
    ADD CONSTRAINT lead_attributions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.leads
    ADD CONSTRAINT leads_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.leave_requests
    ADD CONSTRAINT leave_requests_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.library_article_links
    ADD CONSTRAINT library_article_links_pkey PRIMARY KEY (article_id, document_id);

ALTER TABLE ONLY public.library_document_products
    ADD CONSTRAINT library_document_products_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.library_document_versions
    ADD CONSTRAINT library_document_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.library_documents
    ADD CONSTRAINT library_documents_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.library_downloads
    ADD CONSTRAINT library_downloads_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.mailing_list_contacts
    ADD CONSTRAINT mailing_list_contacts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.mailing_lists
    ADD CONSTRAINT mailing_lists_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.maintenance_equipment
    ADD CONSTRAINT maintenance_equipment_code_key UNIQUE (code);

ALTER TABLE ONLY public.maintenance_equipment
    ADD CONSTRAINT maintenance_equipment_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.maintenance_plans
    ADD CONSTRAINT maintenance_plans_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.maintenance_requests
    ADD CONSTRAINT maintenance_requests_number_key UNIQUE (number);

ALTER TABLE ONLY public.maintenance_requests
    ADD CONSTRAINT maintenance_requests_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.maintenance_work_orders
    ADD CONSTRAINT maintenance_work_orders_number_key UNIQUE (number);

ALTER TABLE ONLY public.maintenance_work_orders
    ADD CONSTRAINT maintenance_work_orders_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.marketing_campaigns
    ADD CONSTRAINT marketing_campaigns_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.mcp_oauth_clients
    ADD CONSTRAINT mcp_oauth_clients_client_id_key UNIQUE (client_id);

ALTER TABLE ONLY public.mcp_oauth_clients
    ADD CONSTRAINT mcp_oauth_clients_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.mcp_oauth_codes
    ADD CONSTRAINT mcp_oauth_codes_code_hash_key UNIQUE (code_hash);

ALTER TABLE ONLY public.mcp_oauth_codes
    ADD CONSTRAINT mcp_oauth_codes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.mdf_claims
    ADD CONSTRAINT mdf_claims_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.mdf_requests
    ADD CONSTRAINT mdf_requests_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.memos
    ADD CONSTRAINT memos_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.message_reactions
    ADD CONSTRAINT message_reactions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_autonomy
    ADD CONSTRAINT nary_autonomy_action_key UNIQUE (action);

ALTER TABLE ONLY public.nary_autonomy
    ADD CONSTRAINT nary_autonomy_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_cadences
    ADD CONSTRAINT nary_cadences_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_owner_topics
    ADD CONSTRAINT nary_owner_topics_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_chat_pauses
    ADD CONSTRAINT nary_chat_pauses_pkey PRIMARY KEY (chat_id);

ALTER TABLE ONLY public.nary_cost_sheets
    ADD CONSTRAINT nary_cost_sheets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_event_followups
    ADD CONSTRAINT nary_event_followups_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_expected_arrivals
    ADD CONSTRAINT nary_expected_arrivals_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_file_links
    ADD CONSTRAINT nary_file_links_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_files
    ADD CONSTRAINT nary_files_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_identity_links
    ADD CONSTRAINT nary_identity_links_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_insights
    ADD CONSTRAINT nary_insights_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_meeting_files
    ADD CONSTRAINT nary_meeting_files_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_meetings
    ADD CONSTRAINT nary_meetings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_memory
    ADD CONSTRAINT nary_memory_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_outbound_log
    ADD CONSTRAINT nary_outbound_log_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_rate_card
    ADD CONSTRAINT nary_rate_card_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_reply_alerts
    ADD CONSTRAINT nary_reply_alerts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_reply_jobs
    ADD CONSTRAINT nary_reply_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_role_assignments
    ADD CONSTRAINT nary_role_assignments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_role_assignments
    ADD CONSTRAINT nary_role_assignments_sop_role_key UNIQUE (sop_role);

ALTER TABLE ONLY public.nary_scores
    ADD CONSTRAINT nary_scores_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.nary_triage_verdicts
    ADD CONSTRAINT nary_triage_verdicts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.note_templates
    ADD CONSTRAINT note_templates_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.offer_letters
    ADD CONSTRAINT offer_letters_number_key UNIQUE (number);

ALTER TABLE ONLY public.offer_letters
    ADD CONSTRAINT offer_letters_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.office_locations
    ADD CONSTRAINT office_locations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.organizations
    ADD CONSTRAINT organizations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.partner_accounts
    ADD CONSTRAINT partner_accounts_agreement_ref_key UNIQUE (agreement_ref);

ALTER TABLE ONLY public.partner_accounts
    ADD CONSTRAINT partner_accounts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.partner_certifications
    ADD CONSTRAINT partner_certifications_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.partner_commissions
    ADD CONSTRAINT partner_commissions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.partner_deal_registrations
    ADD CONSTRAINT partner_deal_registrations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.partner_deal_registrations
    ADD CONSTRAINT partner_deal_registrations_registration_number_key UNIQUE (registration_number);

ALTER TABLE ONLY public.partner_leads
    ADD CONSTRAINT partner_leads_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.partner_module_progress
    ADD CONSTRAINT partner_module_progress_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.partner_resources
    ADD CONSTRAINT partner_resources_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.payment_schedules
    ADD CONSTRAINT payment_schedules_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.payments
    ADD CONSTRAINT payments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.payroll_lines
    ADD CONSTRAINT payroll_lines_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.payroll_runs
    ADD CONSTRAINT payroll_runs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.permissions
    ADD CONSTRAINT permissions_code_key UNIQUE (code);

ALTER TABLE ONLY public.permissions
    ADD CONSTRAINT permissions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.personal_access_tokens
    ADD CONSTRAINT personal_access_tokens_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.personal_access_tokens
    ADD CONSTRAINT personal_access_tokens_token_hash_key UNIQUE (token_hash);

ALTER TABLE ONLY public.portal_registration_requests
    ADD CONSTRAINT portal_registration_requests_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.product_catalog
    ADD CONSTRAINT product_catalog_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.product_catalog
    ADD CONSTRAINT product_catalog_sku_key UNIQUE (sku);

ALTER TABLE ONLY public.product_lines
    ADD CONSTRAINT product_lines_code_key UNIQUE (code);

ALTER TABLE ONLY public.product_lines
    ADD CONSTRAINT product_lines_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.production_orders
    ADD CONSTRAINT production_orders_number_key UNIQUE (number);

ALTER TABLE ONLY public.production_orders
    ADD CONSTRAINT production_orders_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.production_units
    ADD CONSTRAINT production_units_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.production_units
    ADD CONSTRAINT production_units_serial_number_key UNIQUE (serial_number);

ALTER TABLE ONLY public.project_members
    ADD CONSTRAINT project_members_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.project_milestones
    ADD CONSTRAINT project_milestones_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.project_phases
    ADD CONSTRAINT project_phases_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.project_tasks
    ADD CONSTRAINT project_tasks_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_project_number_key UNIQUE (project_number);

ALTER TABLE ONLY public.purchase_order_items
    ADD CONSTRAINT purchase_order_items_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.purchase_orders
    ADD CONSTRAINT purchase_orders_number_key UNIQUE (number);

ALTER TABLE ONLY public.purchase_orders
    ADD CONSTRAINT purchase_orders_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.purchase_rfq_line_responses
    ADD CONSTRAINT purchase_rfq_line_responses_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.purchase_rfq_lines
    ADD CONSTRAINT purchase_rfq_lines_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.purchase_rfq_vendor_invites
    ADD CONSTRAINT purchase_rfq_vendor_invites_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.purchase_rfqs
    ADD CONSTRAINT purchase_rfqs_number_key UNIQUE (number);

ALTER TABLE ONLY public.purchase_rfqs
    ADD CONSTRAINT purchase_rfqs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.qc_records
    ADD CONSTRAINT qc_records_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.quotation_items
    ADD CONSTRAINT quotation_items_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.quotations
    ADD CONSTRAINT quotations_number_key UNIQUE (number);

ALTER TABLE ONLY public.quotations
    ADD CONSTRAINT quotations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.rma_cases
    ADD CONSTRAINT rma_cases_number_key UNIQUE (number);

ALTER TABLE ONLY public.rma_cases
    ADD CONSTRAINT rma_cases_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.rnd_backlog_items
    ADD CONSTRAINT rnd_backlog_items_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.rnd_projects
    ADD CONSTRAINT rnd_projects_code_key UNIQUE (code);

ALTER TABLE ONLY public.rnd_projects
    ADD CONSTRAINT rnd_projects_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.rnd_releases
    ADD CONSTRAINT rnd_releases_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.rnd_sprints
    ADD CONSTRAINT rnd_sprints_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.role_permissions
    ADD CONSTRAINT role_permissions_pkey PRIMARY KEY (role_id, permission_id);

ALTER TABLE ONLY public.roles
    ADD CONSTRAINT roles_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.roles
    ADD CONSTRAINT roles_slug_key UNIQUE (slug);

ALTER TABLE ONLY public.sales_document_assets
    ADD CONSTRAINT sales_document_assets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sales_document_blocks
    ADD CONSTRAINT sales_document_blocks_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sales_document_jobs
    ADD CONSTRAINT sales_document_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sales_document_quotations
    ADD CONSTRAINT sales_document_quotations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sales_documents
    ADD CONSTRAINT sales_documents_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.scheduled_messages
    ADD CONSTRAINT scheduled_messages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sign_audit_logs
    ADD CONSTRAINT sign_audit_logs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sign_documents
    ADD CONSTRAINT sign_documents_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sign_fields
    ADD CONSTRAINT sign_fields_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sign_signers
    ADD CONSTRAINT sign_signers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sign_signers
    ADD CONSTRAINT sign_signers_token_key UNIQUE (token);

ALTER TABLE ONLY public.staff_whatsapp_links
    ADD CONSTRAINT staff_whatsapp_links_chat_id_key UNIQUE (chat_id);

ALTER TABLE ONLY public.staff_whatsapp_links
    ADD CONSTRAINT staff_whatsapp_links_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.stock_locations
    ADD CONSTRAINT stock_locations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.stock_moves
    ADD CONSTRAINT stock_moves_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.system_settings
    ADD CONSTRAINT system_settings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.tax_configs
    ADD CONSTRAINT tax_configs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.tax_rules
    ADD CONSTRAINT tax_rules_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.taxes
    ADD CONSTRAINT taxes_code_key UNIQUE (code);

ALTER TABLE ONLY public.taxes
    ADD CONSTRAINT taxes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.termination_requests
    ADD CONSTRAINT termination_requests_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.time_logs
    ADD CONSTRAINT time_logs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.training_chapters
    ADD CONSTRAINT training_chapters_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.training_modules
    ADD CONSTRAINT training_modules_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.training_quiz_questions
    ADD CONSTRAINT training_quiz_questions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.upload_link_files
    ADD CONSTRAINT upload_link_files_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.upload_links
    ADD CONSTRAINT upload_links_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.upload_links
    ADD CONSTRAINT upload_links_token_hash_key UNIQUE (token_hash);

ALTER TABLE ONLY public.catalog_prices
    ADD CONSTRAINT uq_catalog_price_from UNIQUE (product_id, currency, valid_from);

ALTER TABLE ONLY public.channel_memberships
    ADD CONSTRAINT uq_channel_user UNIQUE (channel_id, user_id);

ALTER TABLE ONLY public.dept_coordinative_links
    ADD CONSTRAINT uq_dept_coord_link UNIQUE (from_department, to_department);

ALTER TABLE ONLY public.document_sequences
    ADD CONSTRAINT uq_doc_seq_type_year_month UNIQUE (sequence_type, year, month);

ALTER TABLE ONLY public.email_image_senders
    ADD CONSTRAINT uq_email_image_senders_user_sender UNIQUE (user_id, sender_email);

ALTER TABLE ONLY public.fiscal_periods
    ADD CONSTRAINT uq_fiscal_period_year_month UNIQUE (year, month);

ALTER TABLE ONLY public.exchange_rates
    ADD CONSTRAINT uq_fx_rate UNIQUE (from_currency, to_currency, effective_date);

ALTER TABLE ONLY public.helpdesk_sla_policies
    ADD CONSTRAINT uq_helpdesk_sla_team_priority UNIQUE (team_id, priority);

ALTER TABLE ONLY public.mailing_list_contacts
    ADD CONSTRAINT uq_mailing_list_email UNIQUE (list_id, email);

ALTER TABLE ONLY public.message_reactions
    ADD CONSTRAINT uq_reaction UNIQUE (message_id, message_source, user_id, emoji);

ALTER TABLE ONLY public.tax_rules
    ADD CONSTRAINT uq_tax_rule_config_code UNIQUE (config_id, code);

ALTER TABLE ONLY public.user_permissions
    ADD CONSTRAINT user_permissions_pkey PRIMARY KEY (user_id, permission_id);

ALTER TABLE ONLY public.user_roles
    ADD CONSTRAINT user_roles_pkey PRIMARY KEY (user_id, role_id);

ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.user_totp_failures
    ADD CONSTRAINT user_totp_failures_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.user_totp
    ADD CONSTRAINT user_totp_pkey PRIMARY KEY (user_id);

ALTER TABLE ONLY public.user_totp_recovery_codes
    ADD CONSTRAINT user_totp_recovery_codes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.vendor_documents
    ADD CONSTRAINT vendor_documents_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.vendor_invoices
    ADD CONSTRAINT vendor_invoices_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.vendors
    ADD CONSTRAINT vendors_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.video_rooms
    ADD CONSTRAINT video_rooms_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.wa_chats
    ADD CONSTRAINT wa_chats_chat_id_key UNIQUE (chat_id);

ALTER TABLE ONLY public.wa_chats
    ADD CONSTRAINT wa_chats_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.wa_contact_links
    ADD CONSTRAINT wa_contact_links_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.wa_hidden_chats
    ADD CONSTRAINT wa_hidden_chats_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.wa_import_chats
    ADD CONSTRAINT wa_import_chats_chat_id_key UNIQUE (chat_id);

ALTER TABLE ONLY public.wa_import_chats
    ADD CONSTRAINT wa_import_chats_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.wa_messages
    ADD CONSTRAINT wa_messages_message_key_key UNIQUE (message_key);

ALTER TABLE ONLY public.wa_messages
    ADD CONSTRAINT wa_messages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.wa_sent_messages
    ADD CONSTRAINT wa_sent_messages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.warehouses
    ADD CONSTRAINT warehouses_code_key UNIQUE (code);

ALTER TABLE ONLY public.warehouses
    ADD CONSTRAINT warehouses_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.warranty_records
    ADD CONSTRAINT warranty_records_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.whatsapp_sessions
    ADD CONSTRAINT whatsapp_sessions_phone_number_key UNIQUE (phone_number);

ALTER TABLE ONLY public.whatsapp_sessions
    ADD CONSTRAINT whatsapp_sessions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.work_order_timesheets
    ADD CONSTRAINT work_order_timesheets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.work_orders
    ADD CONSTRAINT work_orders_number_key UNIQUE (number);

ALTER TABLE ONLY public.work_orders
    ADD CONSTRAINT work_orders_pkey PRIMARY KEY (id);

CREATE UNIQUE INDEX ix_accounts_code ON public.accounts USING btree (code);

CREATE INDEX ix_activities_entity_id ON public.activities USING btree (entity_id);

CREATE INDEX ix_activities_voided_activity ON public.activities_voided USING btree (activity_id);

CREATE INDEX ix_activities_voided_channel ON public.activities_voided USING btree (((snapshot ->> 'channel_message_id'::text))) WHERE (restored_at IS NULL);

CREATE INDEX ix_agent_pending_actions_action ON public.agent_pending_actions USING btree (action);

CREATE INDEX ix_agent_pending_actions_entity_id ON public.agent_pending_actions USING btree (entity_id);

CREATE INDEX ix_agent_pending_actions_status ON public.agent_pending_actions USING btree (status);

CREATE INDEX ix_agent_runs_started ON public.agent_runs USING btree (started_at);

CREATE INDEX ix_agent_steps_run_id ON public.agent_steps USING btree (run_id);

CREATE INDEX ix_ai_conversations_user ON public.ai_conversations USING btree (user_id, updated_at DESC);

CREATE INDEX ix_ai_conversations_user_id ON public.ai_conversations USING btree (user_id);

CREATE INDEX ix_ai_knowledge_chunks_doc_id ON public.ai_knowledge_chunks USING btree (doc_id);

CREATE INDEX ix_ai_messages_conv ON public.ai_messages USING btree (conversation_id, created_at);

CREATE INDEX ix_ai_messages_conversation_id ON public.ai_messages USING btree (conversation_id);

CREATE INDEX ix_ai_tool_jobs_created ON public.ai_tool_jobs USING btree (created_at);

CREATE INDEX ix_ai_usage_created ON public.ai_usage_log USING btree (created_at DESC);

CREATE INDEX ix_ai_usage_log_created ON public.ai_usage_log USING btree (created_at);

CREATE INDEX ix_approval_actions_request ON public.approval_actions USING btree (request_id);

CREATE INDEX ix_approval_policy_steps_policy ON public.approval_policy_steps USING btree (policy_id);

CREATE INDEX ix_approval_requests_entity ON public.approval_requests USING btree (entity_type, entity_id);

CREATE INDEX ix_approval_requests_status ON public.approval_requests USING btree (status);

CREATE INDEX ix_attachments_entity ON public.attachments USING btree (entity_type, entity_id);

CREATE INDEX ix_audit_logs_origin ON public.audit_logs USING btree (origin, created_at DESC);

CREATE INDEX ix_board_columns_scope ON public.board_columns USING btree (scope_type, scope_id);

CREATE INDEX ix_catalog_prices_product_id ON public.catalog_prices USING btree (product_id);

CREATE INDEX ix_catalog_volume_tiers_product_id ON public.catalog_volume_tiers USING btree (product_id);

CREATE INDEX ix_channel_messages_channel_id ON public.channel_messages USING btree (channel_id);

CREATE UNIQUE INDEX ix_channels_slug ON public.channels USING btree (slug);

CREATE INDEX ix_contacts_custom ON public.contacts USING gin (custom);

CREATE INDEX ix_contacts_email ON public.contacts USING btree (email);

CREATE INDEX ix_contacts_lower_email_live ON public.contacts USING btree (lower((email)::text)) WHERE (is_deleted = false);

CREATE INDEX ix_contacts_updated_at ON public.contacts USING btree (updated_at);

CREATE INDEX ix_crm_ai_field_runs_record ON public.crm_ai_field_runs USING btree (object_type, record_id, attribute_key, created_at DESC);

CREATE INDEX ix_crm_attr_defs_object ON public.crm_attribute_defs USING btree (object_type, sort_order);

CREATE INDEX ix_crm_dupe_status ON public.crm_duplicate_candidates USING btree (object_type, status, score DESC);

CREATE INDEX ix_crm_email_messages_created_at ON public.crm_email_messages USING btree (created_at);

CREATE INDEX ix_crm_email_messages_folder_uid ON public.crm_email_messages USING btree (imap_folder, imap_uid);

CREATE INDEX ix_crm_email_messages_mailbox_folder_uid ON public.crm_email_messages USING btree (mailbox_id, imap_folder, imap_uid);

CREATE UNIQUE INDEX ix_crm_email_messages_message_id_header ON public.crm_email_messages USING btree (message_id_header);

CREATE INDEX ix_crm_email_messages_received_at ON public.crm_email_messages USING btree (received_at);

CREATE INDEX ix_crm_email_messages_thread_id ON public.crm_email_messages USING btree (thread_id);

CREATE INDEX ix_crm_email_threads_assigned_to_id ON public.crm_email_threads USING btree (assigned_to_id);

CREATE INDEX ix_crm_email_threads_last_message_at ON public.crm_email_threads USING btree (last_message_at);

CREATE INDEX ix_crm_email_threads_status ON public.crm_email_threads USING btree (status);

CREATE INDEX ix_crm_email_threads_unlinked_last ON public.crm_email_threads USING btree (last_message_at) WHERE (linked_contact_id IS NULL);

CREATE INDEX ix_crm_embeddings_object_type ON public.crm_embeddings USING btree (object_type);

CREATE INDEX ix_crm_embeddings_record_id ON public.crm_embeddings USING btree (record_id);

CREATE INDEX ix_crm_message_copies_message ON public.crm_message_copies USING btree (message_id);

CREATE INDEX ix_crm_reminders_due_at ON public.crm_reminders USING btree (due_at);

CREATE INDEX ix_crm_reminders_entity ON public.crm_reminders USING btree (entity_type, entity_id);

CREATE INDEX ix_crm_reminders_entity_id ON public.crm_reminders USING btree (entity_id);

CREATE INDEX ix_crm_reminders_owner ON public.crm_reminders USING btree (owner_user_id, status, due_at);

CREATE INDEX ix_crm_reminders_owner_user_id ON public.crm_reminders USING btree (owner_user_id);

CREATE INDEX ix_crm_research_people_organization_id ON public.crm_research_people USING btree (organization_id);

CREATE INDEX ix_crm_research_runs_latest ON public.crm_research_runs USING btree (object_type, record_id, created_at DESC);

CREATE INDEX ix_crm_research_runs_record_id ON public.crm_research_runs USING btree (record_id);

CREATE INDEX ix_deal_stage_events_deal_at ON public.deal_stage_events USING btree (deal_id, at);

CREATE INDEX ix_deals_custom ON public.deals USING gin (custom);

CREATE INDEX ix_deals_segment ON public.deals USING btree (segment);

CREATE INDEX ix_direct_messages_thread_id ON public.direct_messages USING btree (thread_id);

CREATE INDEX ix_document_verification_views_verification_id ON public.document_verification_views USING btree (verification_id);

CREATE INDEX ix_document_verifications_entity ON public.document_verifications USING btree (entity_type, entity_id);

CREATE INDEX ix_domain_events_pending ON public.domain_events USING btree (dispatched_at, occurred_at);

CREATE INDEX ix_domain_events_type ON public.domain_events USING btree (type);

CREATE INDEX ix_email_campaigns_status ON public.email_campaigns USING btree (status);

CREATE INDEX ix_employee_allowances_employee_id ON public.employee_allowances USING btree (employee_id);

CREATE INDEX ix_epics_scope ON public.epics USING btree (scope_type, scope_id);

CREATE INDEX ix_eqi_campaign_status ON public.email_queue_items USING btree (campaign_id, status);

CREATE INDEX ix_eqi_tracking_id ON public.email_queue_items USING btree (tracking_id);

CREATE INDEX ix_expenses_employee_id ON public.expenses USING btree (employee_id);

CREATE INDEX ix_face_check_attempts_created_at ON public.face_check_attempts USING btree (created_at);

CREATE INDEX ix_face_descriptors_employee_id ON public.face_descriptors USING btree (employee_id);

CREATE UNIQUE INDEX ix_gl_account_roles_role ON public.gl_account_roles USING btree (role);

CREATE INDEX ix_invoice_refunds_invoice_id ON public.invoice_refunds USING btree (invoice_id);

CREATE INDEX ix_invoices_revision_group ON public.invoices USING btree (revision_group);

CREATE INDEX ix_issue_activity_entity ON public.issue_activity USING btree (entity_type, entity_id);

CREATE INDEX ix_issue_comments_entity ON public.issue_comments USING btree (entity_type, entity_id);

CREATE UNIQUE INDEX ix_journal_entries_entry_number ON public.journal_entries USING btree (entry_number);

CREATE INDEX ix_kb_embeddings_article ON public.kb_embeddings USING btree (article_id);

CREATE INDEX ix_kb_embeddings_article_id ON public.kb_embeddings USING btree (article_id);

CREATE INDEX ix_kb_embeddings_vec ON public.kb_embeddings USING ivfflat (embedding public.vector_cosine_ops) WITH (lists='100');

CREATE INDEX ix_leads_custom ON public.leads USING gin (custom);

CREATE INDEX ix_leads_segment ON public.leads USING btree (segment);

CREATE INDEX ix_library_article_links_doc ON public.library_article_links USING btree (document_id);

CREATE INDEX ix_library_document_products_doc ON public.library_document_products USING btree (document_id);

CREATE INDEX ix_library_document_products_product ON public.library_document_products USING btree (product_id) WHERE (product_id IS NOT NULL);

CREATE INDEX ix_library_document_versions_file_key ON public.library_document_versions USING btree (file_object_key);

CREATE INDEX ix_library_document_versions_status ON public.library_document_versions USING btree (status);

CREATE INDEX ix_library_documents_category ON public.library_documents USING btree (category_id);

CREATE INDEX ix_library_documents_group ON public.library_documents USING btree (language_group_id) WHERE (language_group_id IS NOT NULL);

CREATE INDEX ix_library_documents_owner ON public.library_documents USING btree (owner_id);

CREATE INDEX ix_library_downloads_document ON public.library_downloads USING btree (document_id);

CREATE INDEX ix_library_downloads_sha ON public.library_downloads USING btree (file_sha256);

CREATE UNIQUE INDEX ix_memos_memo_number ON public.memos USING btree (memo_number);

CREATE INDEX ix_memos_recipient_id ON public.memos USING btree (recipient_id);

CREATE INDEX ix_memos_sender_id ON public.memos USING btree (sender_id);

CREATE INDEX ix_message_reactions_message_id ON public.message_reactions USING btree (message_id);

CREATE INDEX ix_mlc_list_id ON public.mailing_list_contacts USING btree (list_id);

CREATE INDEX ix_nary_cadences_due ON public.nary_cadences USING btree (status, due_at);

CREATE INDEX ix_nary_cadences_quotation ON public.nary_cadences USING btree (quotation_id);

CREATE INDEX ix_nary_owner_topics_kind ON public.nary_owner_topics USING btree (kind);

CREATE INDEX ix_nary_owner_topics_ref_id ON public.nary_owner_topics USING btree (ref_id);

CREATE INDEX ix_nary_owner_topics_send_after ON public.nary_owner_topics USING btree (send_after);

CREATE INDEX ix_nary_owner_topics_status ON public.nary_owner_topics USING btree (status);

CREATE INDEX ix_nary_cost_sheets_doc ON public.nary_cost_sheets USING btree (document_id);

CREATE INDEX ix_nary_event_followups_event ON public.nary_event_followups USING btree (event_id);

CREATE INDEX ix_nary_expected_arrivals_phone ON public.nary_expected_arrivals USING btree (phone);

CREATE INDEX ix_nary_expected_arrivals_status ON public.nary_expected_arrivals USING btree (status, due_at);

CREATE INDEX ix_nary_file_links_chat ON public.nary_file_links USING btree (wa_chat_id) WHERE (wa_chat_id IS NOT NULL);

CREATE INDEX ix_nary_file_links_contact ON public.nary_file_links USING btree (contact_id) WHERE (contact_id IS NOT NULL);

CREATE INDEX ix_nary_file_links_deal ON public.nary_file_links USING btree (deal_id) WHERE (deal_id IS NOT NULL);

CREATE INDEX ix_nary_file_links_email ON public.nary_file_links USING btree (email_message_id) WHERE (email_message_id IS NOT NULL);

CREATE INDEX ix_nary_file_links_file ON public.nary_file_links USING btree (file_id);

CREATE INDEX ix_nary_file_links_lead ON public.nary_file_links USING btree (lead_id) WHERE (lead_id IS NOT NULL);

CREATE INDEX ix_nary_file_links_org ON public.nary_file_links USING btree (organization_id) WHERE (organization_id IS NOT NULL);

CREATE INDEX ix_nary_file_links_thread ON public.nary_file_links USING btree (crm_thread_id) WHERE (crm_thread_id IS NOT NULL);

CREATE INDEX ix_nary_identity_links_subject ON public.nary_identity_links USING btree (subject_type, subject_ref);

CREATE INDEX ix_nary_insights_deal ON public.nary_insights USING btree (deal_id, kind);

CREATE INDEX ix_nary_insights_lead ON public.nary_insights USING btree (lead_id, kind);

CREATE INDEX ix_nary_meeting_files_meeting ON public.nary_meeting_files USING btree (meeting_id, created_at);

CREATE INDEX ix_nary_meeting_files_status ON public.nary_meeting_files USING btree (status, next_poll_at);

CREATE INDEX ix_nary_meetings_chat ON public.nary_meetings USING btree (chat_id);

CREATE INDEX ix_nary_meetings_status ON public.nary_meetings USING btree (status, starts_at);

CREATE INDEX ix_nary_memory_subject ON public.nary_memory USING btree (layer, subject_type, subject_id);

CREATE INDEX ix_nary_memory_subject_id ON public.nary_memory USING btree (subject_id);

CREATE INDEX ix_nary_outbound_log_card ON public.nary_outbound_log USING btree (card_id);

CREATE INDEX ix_nary_outbound_log_chat ON public.nary_outbound_log USING btree (chat_id, created_at);

CREATE INDEX ix_nary_outbound_log_contact ON public.nary_outbound_log USING btree (contact_id, created_at);

CREATE INDEX ix_nary_outbound_log_email ON public.nary_outbound_log USING btree (email, created_at);

CREATE INDEX ix_nary_rate_card_role ON public.nary_rate_card USING btree (role, seniority, status);

CREATE INDEX ix_nary_reply_alerts_chat_id ON public.nary_reply_alerts USING btree (chat_id);

CREATE INDEX ix_nary_reply_alerts_thread_id ON public.nary_reply_alerts USING btree (thread_id);

CREATE INDEX ix_nary_reply_jobs_chat ON public.nary_reply_jobs USING btree (chat_id, created_at);

CREATE INDEX ix_nary_reply_jobs_due ON public.nary_reply_jobs USING btree (status, due_at);

CREATE INDEX ix_nary_scores_deal ON public.nary_scores USING btree (deal_id, created_at);

CREATE INDEX ix_nary_scores_lead ON public.nary_scores USING btree (lead_id, created_at);

CREATE INDEX ix_nary_triage_verdicts_subject ON public.nary_triage_verdicts USING btree (subject_type, subject_ref);

CREATE INDEX ix_nary_triage_verdicts_verdict ON public.nary_triage_verdicts USING btree (verdict);

CREATE INDEX ix_organizations_custom ON public.organizations USING gin (custom);

CREATE INDEX ix_organizations_name ON public.organizations USING btree (name);

CREATE INDEX ix_pat_user ON public.personal_access_tokens USING btree (user_id);

CREATE INDEX ix_personal_access_tokens_user_id ON public.personal_access_tokens USING btree (user_id);

CREATE INDEX ix_quotations_revision_group ON public.quotations USING btree (revision_group);

CREATE INDEX ix_sales_document_assets_doc ON public.sales_document_assets USING btree (document_id);

CREATE INDEX ix_sales_document_blocks_doc ON public.sales_document_blocks USING btree (document_id, "position");

CREATE INDEX ix_sales_document_jobs_document ON public.sales_document_jobs USING btree (document_id, created_at DESC);

CREATE INDEX ix_sales_documents_deal ON public.sales_documents USING btree (deal_id);

CREATE INDEX ix_sales_documents_group ON public.sales_documents USING btree (revision_group);

CREATE INDEX ix_staff_whatsapp_links_phone ON public.staff_whatsapp_links USING btree (phone);

CREATE INDEX ix_staff_whatsapp_links_user_id ON public.staff_whatsapp_links USING btree (user_id);

CREATE INDEX ix_stock_moves_component_sku ON public.stock_moves USING btree (component_sku);

CREATE UNIQUE INDEX ix_system_settings_key ON public.system_settings USING btree (key);

CREATE INDEX ix_upload_link_files_link ON public.upload_link_files USING btree (link_id, created_at);

CREATE INDEX ix_upload_links_created_by ON public.upload_links USING btree (created_by_id, created_at DESC);

CREATE INDEX ix_user_sessions_token_hash ON public.user_sessions USING btree (token_hash);

CREATE INDEX ix_user_sessions_family_id ON public.user_sessions USING btree (family_id);

CREATE INDEX ix_user_totp_failures_user_time ON public.user_totp_failures USING btree (user_id, created_at);

CREATE INDEX ix_user_totp_recovery_codes_user ON public.user_totp_recovery_codes USING btree (user_id);

CREATE UNIQUE INDEX ix_users_email ON public.users USING btree (email);

CREATE UNIQUE INDEX ix_video_rooms_room_id ON public.video_rooms USING btree (room_id);

CREATE INDEX ix_wa_chats_phone ON public.wa_chats USING btree (phone);

CREATE INDEX ix_wa_chats_status ON public.wa_chats USING btree (status);

CREATE UNIQUE INDEX ix_wa_contact_links_phone ON public.wa_contact_links USING btree (phone);

CREATE UNIQUE INDEX ix_wa_hidden_chats_chat_id ON public.wa_hidden_chats USING btree (chat_id);

CREATE INDEX ix_wa_messages_chat_ts ON public.wa_messages USING btree (chat_id, wa_ts);

CREATE UNIQUE INDEX ix_wa_sent_messages_message_key ON public.wa_sent_messages USING btree (message_key);

CREATE UNIQUE INDEX uq_approval_requests_pending_entity ON public.approval_requests USING btree (entity_type, entity_id) WHERE ((status)::text = 'pending'::text);

CREATE UNIQUE INDEX uq_crm_embeddings_record ON public.crm_embeddings USING btree (object_type, record_id);

CREATE UNIQUE INDEX uq_crm_reminders_open_rule ON public.crm_reminders USING btree (rule_key) WHERE (((status)::text = 'open'::text) AND (rule_key IS NOT NULL));

CREATE UNIQUE INDEX uq_crm_research_people_person ON public.crm_research_people USING btree (organization_id, name_key);

CREATE UNIQUE INDEX uq_email_messages_account_folder_uid ON public.email_messages USING btree (account_id, folder, imap_uid) WHERE (imap_uid IS NOT NULL);

CREATE UNIQUE INDEX uq_journal_entries_auto_source ON public.journal_entries USING btree (source, source_id) WHERE ((is_deleted = false) AND (source IS NOT NULL) AND ((source)::text <> ALL (ARRAY[('manual'::character varying)::text, ('reversal'::character varying)::text, ('fx_auto'::character varying)::text, ('asset_depreciation'::character varying)::text])));

CREATE UNIQUE INDEX uq_payroll_runs_period ON public.payroll_runs USING btree (year, month) WHERE ((status)::text <> 'cancelled'::text);

CREATE UNIQUE INDEX uq_sales_document_quotations ON public.sales_document_quotations USING btree (document_id, quotation_id);

CREATE UNIQUE INDEX uq_staff_whatsapp_links_primary ON public.staff_whatsapp_links USING btree (user_id) WHERE is_primary;

CREATE UNIQUE INDEX uq_vendor_invoices_vendor_number ON public.vendor_invoices USING btree (vendor_id, lower((vendor_invoice_number)::text)) WHERE (is_deleted = false);

CREATE UNIQUE INDEX ux_agent_pending_actions_trigger_key ON public.agent_pending_actions USING btree (trigger_key) WHERE (trigger_key IS NOT NULL);

CREATE UNIQUE INDEX ux_ai_knowledge_docs_system_key ON public.ai_knowledge_docs USING btree (system_key) WHERE (system_key IS NOT NULL);

CREATE UNIQUE INDEX ux_crm_attr_defs_object_key ON public.crm_attribute_defs USING btree (object_type, key);

CREATE UNIQUE INDEX ux_crm_dupe_pair ON public.crm_duplicate_candidates USING btree (object_type, record_a_id, record_b_id);

CREATE UNIQUE INDEX ux_crm_mailboxes_default ON public.crm_mailboxes USING btree (is_default) WHERE is_default;

CREATE UNIQUE INDEX ux_doc_categories_key ON public.doc_categories USING btree (key);

CREATE UNIQUE INDEX ux_gitlab_user_links_gl ON public.gitlab_user_links USING btree (gitlab_config_id, gitlab_user_id) WHERE (gitlab_user_id IS NOT NULL);

CREATE UNIQUE INDEX ux_gitlab_user_links_user ON public.gitlab_user_links USING btree (gitlab_config_id, user_id) WHERE (user_id IS NOT NULL);

CREATE UNIQUE INDEX ux_issue_comments_gitlab_note ON public.issue_comments USING btree (gitlab_note_id) WHERE (gitlab_note_id IS NOT NULL);

CREATE UNIQUE INDEX ux_library_document_versions_label ON public.library_document_versions USING btree (document_id, label);

CREATE UNIQUE INDEX ux_library_downloads_fingerprint ON public.library_downloads USING btree (fingerprint);

CREATE UNIQUE INDEX ux_nary_file_links_ref ON public.nary_file_links USING btree (source_ref);

CREATE UNIQUE INDEX ux_nary_files_sha256 ON public.nary_files USING btree (sha256);

CREATE UNIQUE INDEX ux_nary_meeting_files_wa_message ON public.nary_meeting_files USING btree (wa_message_key) WHERE (wa_message_key IS NOT NULL);

CREATE UNIQUE INDEX ux_nary_reply_jobs_message_key ON public.nary_reply_jobs USING btree (message_key);

CREATE UNIQUE INDEX ux_nary_reply_jobs_pending_chat ON public.nary_reply_jobs USING btree (chat_id) WHERE ((status)::text = 'pending'::text);

CREATE UNIQUE INDEX ux_pat_hash ON public.personal_access_tokens USING btree (token_hash);

CREATE UNIQUE INDEX ux_pat_refresh ON public.personal_access_tokens USING btree (refresh_token_hash) WHERE (refresh_token_hash IS NOT NULL);

CREATE UNIQUE INDEX ux_production_units_serial_key ON public.production_units USING btree (public.production_serial_key((serial_number)::text));

CREATE UNIQUE INDEX ux_rnd_backlog_items_gitlab_issue ON public.rnd_backlog_items USING btree (gitlab_repository_id, gitlab_issue_iid) WHERE (gitlab_repository_id IS NOT NULL);

ALTER TABLE ONLY public.accounts
    ADD CONSTRAINT accounts_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.activities
    ADD CONSTRAINT activities_assigned_to_id_fkey FOREIGN KEY (assigned_to_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.activities
    ADD CONSTRAINT activities_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.activities
    ADD CONSTRAINT activities_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.activities
    ADD CONSTRAINT activities_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.activities
    ADD CONSTRAINT activities_lead_id_fkey FOREIGN KEY (lead_id) REFERENCES public.leads(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.activities_voided
    ADD CONSTRAINT activities_voided_restored_by_id_fkey FOREIGN KEY (restored_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.activities_voided
    ADD CONSTRAINT activities_voided_voided_by_id_fkey FOREIGN KEY (voided_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_pending_actions
    ADD CONSTRAINT agent_pending_actions_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_pending_actions
    ADD CONSTRAINT agent_pending_actions_assignee_id_fkey FOREIGN KEY (assignee_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_pending_actions
    ADD CONSTRAINT agent_pending_actions_decided_by_id_fkey FOREIGN KEY (decided_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_pending_actions
    ADD CONSTRAINT agent_pending_actions_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.agent_runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_steps
    ADD CONSTRAINT agent_steps_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.agent_runs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.ai_conversations
    ADD CONSTRAINT ai_conversations_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.ai_knowledge_chunks
    ADD CONSTRAINT ai_knowledge_chunks_doc_id_fkey FOREIGN KEY (doc_id) REFERENCES public.ai_knowledge_docs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.ai_knowledge_docs
    ADD CONSTRAINT ai_knowledge_docs_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.ai_messages
    ADD CONSTRAINT ai_messages_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.ai_conversations(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.ai_tool_jobs
    ADD CONSTRAINT ai_tool_jobs_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.ai_usage_log
    ADD CONSTRAINT ai_usage_log_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.agent_runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.ai_usage_log
    ADD CONSTRAINT ai_usage_log_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.applicants
    ADD CONSTRAINT applicants_assigned_to_id_fkey FOREIGN KEY (assigned_to_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.applicants
    ADD CONSTRAINT applicants_job_opening_id_fkey FOREIGN KEY (job_opening_id) REFERENCES public.job_openings(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.applicants
    ADD CONSTRAINT applicants_referred_by_id_fkey FOREIGN KEY (referred_by_id) REFERENCES public.employees(id);

ALTER TABLE ONLY public.approval_actions
    ADD CONSTRAINT approval_actions_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.approval_actions
    ADD CONSTRAINT approval_actions_request_id_fkey FOREIGN KEY (request_id) REFERENCES public.approval_requests(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.approval_policies
    ADD CONSTRAINT approval_policies_updated_by_id_fkey FOREIGN KEY (updated_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.approval_policy_steps
    ADD CONSTRAINT approval_policy_steps_policy_id_fkey FOREIGN KEY (policy_id) REFERENCES public.approval_policies(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.approval_policy_steps
    ADD CONSTRAINT approval_policy_steps_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.approval_requests
    ADD CONSTRAINT approval_requests_policy_id_fkey FOREIGN KEY (policy_id) REFERENCES public.approval_policies(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.approval_requests
    ADD CONSTRAINT approval_requests_submitted_by_id_fkey FOREIGN KEY (submitted_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.asset_categories
    ADD CONSTRAINT asset_categories_gl_accumulated_account_id_fkey FOREIGN KEY (gl_accumulated_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.asset_categories
    ADD CONSTRAINT asset_categories_gl_asset_account_id_fkey FOREIGN KEY (gl_asset_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.asset_categories
    ADD CONSTRAINT asset_categories_gl_depreciation_account_id_fkey FOREIGN KEY (gl_depreciation_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.asset_depreciation_lines
    ADD CONSTRAINT asset_depreciation_lines_asset_id_fkey FOREIGN KEY (asset_id) REFERENCES public.fixed_assets(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.asset_depreciation_lines
    ADD CONSTRAINT asset_depreciation_lines_journal_entry_id_fkey FOREIGN KEY (journal_entry_id) REFERENCES public.journal_entries(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.asset_disposals
    ADD CONSTRAINT asset_disposals_asset_id_fkey FOREIGN KEY (asset_id) REFERENCES public.fixed_assets(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.asset_disposals
    ADD CONSTRAINT asset_disposals_bank_account_id_fkey FOREIGN KEY (bank_account_id) REFERENCES public.bank_accounts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.asset_disposals
    ADD CONSTRAINT asset_disposals_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.asset_disposals
    ADD CONSTRAINT asset_disposals_journal_entry_id_fkey FOREIGN KEY (journal_entry_id) REFERENCES public.journal_entries(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.attachments
    ADD CONSTRAINT attachments_uploaded_by_id_fkey FOREIGN KEY (uploaded_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.attendance_records
    ADD CONSTRAINT attendance_records_employee_id_fkey FOREIGN KEY (employee_id) REFERENCES public.employees(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.attendance_records
    ADD CONSTRAINT attendance_records_recorded_by_id_fkey FOREIGN KEY (recorded_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.audit_logs
    ADD CONSTRAINT audit_logs_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.bank_accounts
    ADD CONSTRAINT bank_accounts_gl_account_id_fkey FOREIGN KEY (gl_account_id) REFERENCES public.accounts(id);

ALTER TABLE ONLY public.bank_statement_lines
    ADD CONSTRAINT bank_statement_lines_matched_by_id_fkey FOREIGN KEY (matched_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.bank_statement_lines
    ADD CONSTRAINT bank_statement_lines_matched_journal_line_id_fkey FOREIGN KEY (matched_journal_line_id) REFERENCES public.journal_lines(id);

ALTER TABLE ONLY public.bank_statement_lines
    ADD CONSTRAINT bank_statement_lines_statement_id_fkey FOREIGN KEY (statement_id) REFERENCES public.bank_statements(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.bank_statements
    ADD CONSTRAINT bank_statements_bank_account_id_fkey FOREIGN KEY (bank_account_id) REFERENCES public.bank_accounts(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.bank_statements
    ADD CONSTRAINT bank_statements_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.bom_items
    ADD CONSTRAINT bom_items_bom_id_fkey FOREIGN KEY (bom_id) REFERENCES public.boms(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.boms
    ADD CONSTRAINT boms_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.boms
    ADD CONSTRAINT boms_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.boms
    ADD CONSTRAINT boms_product_id_fkey FOREIGN KEY (product_id) REFERENCES public.hardware_products(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.budget_lines
    ADD CONSTRAINT budget_lines_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.budget_lines
    ADD CONSTRAINT budget_lines_budget_id_fkey FOREIGN KEY (budget_id) REFERENCES public.budgets(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.budgets
    ADD CONSTRAINT budgets_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.budgets
    ADD CONSTRAINT budgets_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.budgets
    ADD CONSTRAINT budgets_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.catalog_prices
    ADD CONSTRAINT catalog_prices_product_id_fkey FOREIGN KEY (product_id) REFERENCES public.product_catalog(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.catalog_volume_tiers
    ADD CONSTRAINT catalog_volume_tiers_product_id_fkey FOREIGN KEY (product_id) REFERENCES public.product_catalog(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.channel_memberships
    ADD CONSTRAINT channel_memberships_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.channels(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.channel_memberships
    ADD CONSTRAINT channel_memberships_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.channel_messages
    ADD CONSTRAINT channel_messages_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.channels(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.channel_messages
    ADD CONSTRAINT channel_messages_reply_to_id_fkey FOREIGN KEY (reply_to_id) REFERENCES public.channel_messages(id);

ALTER TABLE ONLY public.channel_messages
    ADD CONSTRAINT channel_messages_sender_id_fkey FOREIGN KEY (sender_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.channel_messages
    ADD CONSTRAINT channel_messages_thread_root_id_fkey FOREIGN KEY (thread_root_id) REFERENCES public.channel_messages(id);

ALTER TABLE ONLY public.channels
    ADD CONSTRAINT channels_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.clock_events
    ADD CONSTRAINT clock_events_attendance_record_id_fkey FOREIGN KEY (attendance_record_id) REFERENCES public.attendance_records(id);

ALTER TABLE ONLY public.clock_events
    ADD CONSTRAINT clock_events_employee_id_fkey FOREIGN KEY (employee_id) REFERENCES public.employees(id);

ALTER TABLE ONLY public.clock_events
    ADD CONSTRAINT clock_events_kiosk_device_id_fkey FOREIGN KEY (kiosk_device_id) REFERENCES public.kiosk_devices(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.clock_events
    ADD CONSTRAINT clock_events_office_location_id_fkey FOREIGN KEY (office_location_id) REFERENCES public.office_locations(id);

ALTER TABLE ONLY public.clock_events
    ADD CONSTRAINT clock_events_override_by_id_fkey FOREIGN KEY (override_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.component_inventory
    ADD CONSTRAINT component_inventory_location_id_fkey FOREIGN KEY (location_id) REFERENCES public.stock_locations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.component_inventory
    ADD CONSTRAINT component_inventory_updated_by_id_fkey FOREIGN KEY (updated_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.contacts
    ADD CONSTRAINT contacts_assigned_to_id_fkey FOREIGN KEY (assigned_to_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.contacts
    ADD CONSTRAINT contacts_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.contacts
    ADD CONSTRAINT contacts_organization_id_fkey FOREIGN KEY (organization_id) REFERENCES public.organizations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.content_calendar
    ADD CONSTRAINT content_calendar_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.content_calendar
    ADD CONSTRAINT content_calendar_assigned_to_id_fkey FOREIGN KEY (assigned_to_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.content_calendar
    ADD CONSTRAINT content_calendar_campaign_id_fkey FOREIGN KEY (campaign_id) REFERENCES public.marketing_campaigns(id);

ALTER TABLE ONLY public.contractor_requests
    ADD CONSTRAINT contractor_requests_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.contractor_requests
    ADD CONSTRAINT contractor_requests_preferred_vendor_id_fkey FOREIGN KEY (preferred_vendor_id) REFERENCES public.vendors(id);

ALTER TABLE ONLY public.contractor_requests
    ADD CONSTRAINT contractor_requests_purchase_order_id_fkey FOREIGN KEY (purchase_order_id) REFERENCES public.purchase_orders(id);

ALTER TABLE ONLY public.contractor_requests
    ADD CONSTRAINT contractor_requests_requested_by_id_fkey FOREIGN KEY (requested_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.contractor_requests
    ADD CONSTRAINT contractor_requests_selected_vendor_id_fkey FOREIGN KEY (selected_vendor_id) REFERENCES public.vendors(id);

ALTER TABLE ONLY public.contractor_requests
    ADD CONSTRAINT contractor_requests_work_order_id_fkey FOREIGN KEY (work_order_id) REFERENCES public.work_orders(id);

ALTER TABLE ONLY public.contracts
    ADD CONSTRAINT contracts_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.contracts
    ADD CONSTRAINT contracts_customer_contact_id_fkey FOREIGN KEY (customer_contact_id) REFERENCES public.contacts(id);

ALTER TABLE ONLY public.contracts
    ADD CONSTRAINT contracts_customer_org_id_fkey FOREIGN KEY (customer_org_id) REFERENCES public.organizations(id);

ALTER TABLE ONLY public.contracts
    ADD CONSTRAINT contracts_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id);

ALTER TABLE ONLY public.contracts
    ADD CONSTRAINT contracts_quotation_id_fkey FOREIGN KEY (quotation_id) REFERENCES public.quotations(id);

ALTER TABLE ONLY public.contracts
    ADD CONSTRAINT contracts_signed_by_company_id_fkey FOREIGN KEY (signed_by_company_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.crm_ai_field_runs
    ADD CONSTRAINT crm_ai_field_runs_run_by_id_fkey FOREIGN KEY (run_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_attribute_defs
    ADD CONSTRAINT crm_attribute_defs_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_duplicate_candidates
    ADD CONSTRAINT crm_duplicate_candidates_resolved_by_id_fkey FOREIGN KEY (resolved_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_email_attachments
    ADD CONSTRAINT crm_email_attachments_message_id_fkey FOREIGN KEY (message_id) REFERENCES public.crm_email_messages(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.crm_email_messages
    ADD CONSTRAINT crm_email_messages_mailbox_id_fkey FOREIGN KEY (mailbox_id) REFERENCES public.crm_mailboxes(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_email_messages
    ADD CONSTRAINT crm_email_messages_sent_by_id_fkey FOREIGN KEY (sent_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.crm_email_messages
    ADD CONSTRAINT crm_email_messages_thread_id_fkey FOREIGN KEY (thread_id) REFERENCES public.crm_email_threads(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.crm_email_threads
    ADD CONSTRAINT crm_email_threads_assigned_by_id_fkey FOREIGN KEY (assigned_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_email_threads
    ADD CONSTRAINT crm_email_threads_assigned_to_id_fkey FOREIGN KEY (assigned_to_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_email_threads
    ADD CONSTRAINT crm_email_threads_linked_contact_id_fkey FOREIGN KEY (linked_contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_email_threads
    ADD CONSTRAINT crm_email_threads_linked_deal_id_fkey FOREIGN KEY (linked_deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_email_threads
    ADD CONSTRAINT crm_email_threads_linked_lead_id_fkey FOREIGN KEY (linked_lead_id) REFERENCES public.leads(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_email_threads
    ADD CONSTRAINT crm_email_threads_linked_org_id_fkey FOREIGN KEY (linked_org_id) REFERENCES public.organizations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_email_threads
    ADD CONSTRAINT crm_email_threads_mailbox_id_fkey FOREIGN KEY (mailbox_id) REFERENCES public.crm_mailboxes(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_message_copies
    ADD CONSTRAINT crm_message_copies_mailbox_id_fkey FOREIGN KEY (mailbox_id) REFERENCES public.crm_mailboxes(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.crm_message_copies
    ADD CONSTRAINT crm_message_copies_message_id_fkey FOREIGN KEY (message_id) REFERENCES public.crm_email_messages(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.crm_reminders
    ADD CONSTRAINT crm_reminders_completed_by_id_fkey FOREIGN KEY (completed_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_reminders
    ADD CONSTRAINT crm_reminders_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_reminders
    ADD CONSTRAINT crm_reminders_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.crm_research_people
    ADD CONSTRAINT crm_research_people_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_research_people
    ADD CONSTRAINT crm_research_people_matched_contact_id_fkey FOREIGN KEY (matched_contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_research_people
    ADD CONSTRAINT crm_research_people_organization_id_fkey FOREIGN KEY (organization_id) REFERENCES public.organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.crm_research_people
    ADD CONSTRAINT crm_research_people_resolved_by_id_fkey FOREIGN KEY (resolved_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_research_people
    ADD CONSTRAINT crm_research_people_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.crm_research_runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.crm_research_runs
    ADD CONSTRAINT crm_research_runs_requested_by_id_fkey FOREIGN KEY (requested_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.deal_stage_events
    ADD CONSTRAINT deal_stage_events_by_id_fkey FOREIGN KEY (by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.deal_stage_events
    ADD CONSTRAINT deal_stage_events_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.deals
    ADD CONSTRAINT deals_assigned_to_id_fkey FOREIGN KEY (assigned_to_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.deals
    ADD CONSTRAINT deals_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.deals
    ADD CONSTRAINT deals_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.deals
    ADD CONSTRAINT deals_deal_registration_id_fkey FOREIGN KEY (deal_registration_id) REFERENCES public.partner_deal_registrations(id);

ALTER TABLE ONLY public.deals
    ADD CONSTRAINT deals_lead_id_fkey FOREIGN KEY (lead_id) REFERENCES public.leads(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.deals
    ADD CONSTRAINT deals_mailbox_id_fkey FOREIGN KEY (mailbox_id) REFERENCES public.crm_mailboxes(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.deals
    ADD CONSTRAINT deals_organization_id_fkey FOREIGN KEY (organization_id) REFERENCES public.organizations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.dept_coordinative_links
    ADD CONSTRAINT dept_coordinative_links_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.direct_messages
    ADD CONSTRAINT direct_messages_receiver_id_fkey FOREIGN KEY (receiver_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.direct_messages
    ADD CONSTRAINT direct_messages_reply_to_id_fkey FOREIGN KEY (reply_to_id) REFERENCES public.direct_messages(id);

ALTER TABLE ONLY public.direct_messages
    ADD CONSTRAINT direct_messages_sender_id_fkey FOREIGN KEY (sender_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.direct_messages
    ADD CONSTRAINT direct_messages_thread_root_id_fkey FOREIGN KEY (thread_root_id) REFERENCES public.direct_messages(id);

ALTER TABLE ONLY public.document_verification_views
    ADD CONSTRAINT document_verification_views_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.document_verification_views
    ADD CONSTRAINT document_verification_views_verification_id_fkey FOREIGN KEY (verification_id) REFERENCES public.document_verifications(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.document_verifications
    ADD CONSTRAINT document_verifications_approval_request_id_fkey FOREIGN KEY (approval_request_id) REFERENCES public.approval_requests(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.document_verifications
    ADD CONSTRAINT document_verifications_issued_by_id_fkey FOREIGN KEY (issued_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.document_verifications
    ADD CONSTRAINT document_verifications_revoked_by_id_fkey FOREIGN KEY (revoked_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.document_verifications
    ADD CONSTRAINT document_verifications_sign_document_id_fkey FOREIGN KEY (sign_document_id) REFERENCES public.sign_documents(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.document_verifications
    ADD CONSTRAINT document_verifications_superseded_by_id_fkey FOREIGN KEY (superseded_by_id) REFERENCES public.document_verifications(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.domain_events
    ADD CONSTRAINT domain_events_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.email_accounts
    ADD CONSTRAINT email_accounts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.email_campaigns
    ADD CONSTRAINT email_campaigns_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.email_campaigns
    ADD CONSTRAINT email_campaigns_sender_user_id_fkey FOREIGN KEY (sender_user_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.email_image_senders
    ADD CONSTRAINT email_image_senders_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.email_messages
    ADD CONSTRAINT email_messages_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.email_accounts(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.email_messages
    ADD CONSTRAINT email_messages_linked_contact_id_fkey FOREIGN KEY (linked_contact_id) REFERENCES public.contacts(id);

ALTER TABLE ONLY public.email_messages
    ADD CONSTRAINT email_messages_linked_deal_id_fkey FOREIGN KEY (linked_deal_id) REFERENCES public.deals(id);

ALTER TABLE ONLY public.email_queue_items
    ADD CONSTRAINT email_queue_items_campaign_id_fkey FOREIGN KEY (campaign_id) REFERENCES public.email_campaigns(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.email_queue_items
    ADD CONSTRAINT email_queue_items_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.mailing_list_contacts(id);

ALTER TABLE ONLY public.email_queue_items
    ADD CONSTRAINT email_queue_items_list_id_fkey FOREIGN KEY (list_id) REFERENCES public.mailing_lists(id);

ALTER TABLE ONLY public.email_suppressions
    ADD CONSTRAINT email_suppressions_campaign_id_fkey FOREIGN KEY (campaign_id) REFERENCES public.email_campaigns(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.employee_allowances
    ADD CONSTRAINT employee_allowances_employee_id_fkey FOREIGN KEY (employee_id) REFERENCES public.employees(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.employees
    ADD CONSTRAINT employees_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.employees
    ADD CONSTRAINT employees_reports_to_id_fkey FOREIGN KEY (reports_to_id) REFERENCES public.employees(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.employees
    ADD CONSTRAINT employees_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.epics
    ADD CONSTRAINT epics_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.exchange_rates
    ADD CONSTRAINT exchange_rates_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.expenses
    ADD CONSTRAINT expenses_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.expenses
    ADD CONSTRAINT expenses_bank_account_id_fkey FOREIGN KEY (bank_account_id) REFERENCES public.bank_accounts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.expenses
    ADD CONSTRAINT expenses_employee_id_fkey FOREIGN KEY (employee_id) REFERENCES public.employees(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.expenses
    ADD CONSTRAINT expenses_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.expenses
    ADD CONSTRAINT expenses_submitted_by_id_fkey FOREIGN KEY (submitted_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.expenses
    ADD CONSTRAINT expenses_vendor_id_fkey FOREIGN KEY (vendor_id) REFERENCES public.vendors(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.expenses
    ADD CONSTRAINT expenses_work_order_id_fkey FOREIGN KEY (work_order_id) REFERENCES public.work_orders(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.face_check_attempts
    ADD CONSTRAINT face_check_attempts_employee_id_fkey FOREIGN KEY (employee_id) REFERENCES public.employees(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.face_check_attempts
    ADD CONSTRAINT face_check_attempts_kiosk_device_id_fkey FOREIGN KEY (kiosk_device_id) REFERENCES public.kiosk_devices(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.face_check_attempts
    ADD CONSTRAINT face_check_attempts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.face_descriptors
    ADD CONSTRAINT face_descriptors_employee_id_fkey FOREIGN KEY (employee_id) REFERENCES public.employees(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.face_descriptors
    ADD CONSTRAINT face_descriptors_enrolled_by_id_fkey FOREIGN KEY (enrolled_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.face_enrollments
    ADD CONSTRAINT face_enrollments_employee_id_fkey FOREIGN KEY (employee_id) REFERENCES public.employees(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.face_enrollments
    ADD CONSTRAINT face_enrollments_enrolled_by_id_fkey FOREIGN KEY (enrolled_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.fiscal_periods
    ADD CONSTRAINT fiscal_periods_locked_by_id_fkey FOREIGN KEY (locked_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.fixed_assets
    ADD CONSTRAINT fixed_assets_category_id_fkey FOREIGN KEY (category_id) REFERENCES public.asset_categories(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.fixed_assets
    ADD CONSTRAINT fixed_assets_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.fixed_assets
    ADD CONSTRAINT fixed_assets_purchase_order_id_fkey FOREIGN KEY (purchase_order_id) REFERENCES public.purchase_orders(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.fixed_assets
    ADD CONSTRAINT fixed_assets_vendor_id_fkey FOREIGN KEY (vendor_id) REFERENCES public.vendors(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.channel_messages
    ADD CONSTRAINT fk_cm_memo_id FOREIGN KEY (memo_id) REFERENCES public.memos(id);

ALTER TABLE ONLY public.component_inventory
    ADD CONSTRAINT fk_comp_inv_location FOREIGN KEY (location_id) REFERENCES public.stock_locations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.direct_messages
    ADD CONSTRAINT fk_dm_memo_id FOREIGN KEY (memo_id) REFERENCES public.memos(id);

ALTER TABLE ONLY public.expenses
    ADD CONSTRAINT fk_expenses_employee FOREIGN KEY (employee_id) REFERENCES public.employees(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.goods_receipts
    ADD CONSTRAINT fk_gr_location FOREIGN KEY (location_id) REFERENCES public.stock_locations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.kb_articles
    ADD CONSTRAINT fk_kb_articles_owner FOREIGN KEY (owner_signed_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.library_documents
    ADD CONSTRAINT fk_library_documents_current_version FOREIGN KEY (current_version_id) REFERENCES public.library_document_versions(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.purchase_order_items
    ADD CONSTRAINT fk_po_item_rfq_line FOREIGN KEY (rfq_line_id) REFERENCES public.purchase_rfq_lines(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.purchase_orders
    ADD CONSTRAINT fk_po_rfq FOREIGN KEY (rfq_id) REFERENCES public.purchase_rfqs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.production_orders
    ADD CONSTRAINT fk_production_orders_invoice FOREIGN KEY (invoice_id) REFERENCES public.invoices(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.rnd_projects
    ADD CONSTRAINT fk_rnd_projects_hw_product FOREIGN KEY (hardware_product_id) REFERENCES public.hardware_products(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.fleet_assignments
    ADD CONSTRAINT fleet_assignments_assigned_by_id_fkey FOREIGN KEY (assigned_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.fleet_assignments
    ADD CONSTRAINT fleet_assignments_employee_id_fkey FOREIGN KEY (employee_id) REFERENCES public.employees(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.fleet_assignments
    ADD CONSTRAINT fleet_assignments_vehicle_id_fkey FOREIGN KEY (vehicle_id) REFERENCES public.fleet_vehicles(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.fleet_fuel_logs
    ADD CONSTRAINT fleet_fuel_logs_logged_by_id_fkey FOREIGN KEY (logged_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.fleet_fuel_logs
    ADD CONSTRAINT fleet_fuel_logs_vehicle_id_fkey FOREIGN KEY (vehicle_id) REFERENCES public.fleet_vehicles(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.fleet_odometer_logs
    ADD CONSTRAINT fleet_odometer_logs_logged_by_id_fkey FOREIGN KEY (logged_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.fleet_odometer_logs
    ADD CONSTRAINT fleet_odometer_logs_vehicle_id_fkey FOREIGN KEY (vehicle_id) REFERENCES public.fleet_vehicles(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.fleet_service_records
    ADD CONSTRAINT fleet_service_records_logged_by_id_fkey FOREIGN KEY (logged_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.fleet_service_records
    ADD CONSTRAINT fleet_service_records_vehicle_id_fkey FOREIGN KEY (vehicle_id) REFERENCES public.fleet_vehicles(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.gitlab_issue_queue
    ADD CONSTRAINT gitlab_issue_queue_repository_id_fkey FOREIGN KEY (repository_id) REFERENCES public.gitlab_repositories(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.gitlab_pipeline_runs
    ADD CONSTRAINT gitlab_pipeline_runs_repository_id_fkey FOREIGN KEY (repository_id) REFERENCES public.gitlab_repositories(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.gitlab_repositories
    ADD CONSTRAINT gitlab_repositories_gitlab_config_id_fkey FOREIGN KEY (gitlab_config_id) REFERENCES public.gitlab_configs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.gitlab_repositories
    ADD CONSTRAINT gitlab_repositories_rnd_project_id_fkey FOREIGN KEY (rnd_project_id) REFERENCES public.rnd_projects(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.gitlab_user_links
    ADD CONSTRAINT gitlab_user_links_gitlab_config_id_fkey FOREIGN KEY (gitlab_config_id) REFERENCES public.gitlab_configs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.gitlab_user_links
    ADD CONSTRAINT gitlab_user_links_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.gitlab_webhook_events
    ADD CONSTRAINT gitlab_webhook_events_linked_backlog_item_id_fkey FOREIGN KEY (linked_backlog_item_id) REFERENCES public.rnd_backlog_items(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.gitlab_webhook_events
    ADD CONSTRAINT gitlab_webhook_events_repository_id_fkey FOREIGN KEY (repository_id) REFERENCES public.gitlab_repositories(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.gl_account_roles
    ADD CONSTRAINT gl_account_roles_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.goods_receipts
    ADD CONSTRAINT goods_receipts_location_id_fkey FOREIGN KEY (location_id) REFERENCES public.stock_locations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.goods_receipts
    ADD CONSTRAINT goods_receipts_po_id_fkey FOREIGN KEY (po_id) REFERENCES public.purchase_orders(id);

ALTER TABLE ONLY public.goods_receipts
    ADD CONSTRAINT goods_receipts_received_by_id_fkey FOREIGN KEY (received_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.google_accounts
    ADD CONSTRAINT google_accounts_connected_by_id_fkey FOREIGN KEY (connected_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.hardware_products
    ADD CONSTRAINT hardware_products_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.hardware_products
    ADD CONSTRAINT hardware_products_current_bom_id_fkey FOREIGN KEY (current_bom_id) REFERENCES public.boms(id);

ALTER TABLE ONLY public.hardware_products
    ADD CONSTRAINT hardware_products_product_line_id_fkey FOREIGN KEY (product_line_id) REFERENCES public.product_lines(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.helpdesk_categories
    ADD CONSTRAINT helpdesk_categories_team_id_fkey FOREIGN KEY (team_id) REFERENCES public.helpdesk_teams(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.helpdesk_messages
    ADD CONSTRAINT helpdesk_messages_author_user_id_fkey FOREIGN KEY (author_user_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.helpdesk_messages
    ADD CONSTRAINT helpdesk_messages_ticket_id_fkey FOREIGN KEY (ticket_id) REFERENCES public.helpdesk_tickets(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.helpdesk_sla_policies
    ADD CONSTRAINT helpdesk_sla_policies_team_id_fkey FOREIGN KEY (team_id) REFERENCES public.helpdesk_teams(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.helpdesk_tickets
    ADD CONSTRAINT helpdesk_tickets_assignee_id_fkey FOREIGN KEY (assignee_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.helpdesk_tickets
    ADD CONSTRAINT helpdesk_tickets_category_id_fkey FOREIGN KEY (category_id) REFERENCES public.helpdesk_categories(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.helpdesk_tickets
    ADD CONSTRAINT helpdesk_tickets_sla_policy_id_fkey FOREIGN KEY (sla_policy_id) REFERENCES public.helpdesk_sla_policies(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.helpdesk_tickets
    ADD CONSTRAINT helpdesk_tickets_team_id_fkey FOREIGN KEY (team_id) REFERENCES public.helpdesk_teams(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.hiring_requests
    ADD CONSTRAINT hiring_requests_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.hiring_requests
    ADD CONSTRAINT hiring_requests_reports_to_employee_id_fkey FOREIGN KEY (reports_to_employee_id) REFERENCES public.employees(id);

ALTER TABLE ONLY public.hiring_requests
    ADD CONSTRAINT hiring_requests_requested_by_id_fkey FOREIGN KEY (requested_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.invoice_items
    ADD CONSTRAINT invoice_items_invoice_id_fkey FOREIGN KEY (invoice_id) REFERENCES public.invoices(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.invoice_items
    ADD CONSTRAINT invoice_items_parent_item_id_fkey FOREIGN KEY (parent_item_id) REFERENCES public.invoice_items(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.invoice_items
    ADD CONSTRAINT invoice_items_product_id_fkey FOREIGN KEY (product_id) REFERENCES public.product_catalog(id);

ALTER TABLE ONLY public.invoice_items
    ADD CONSTRAINT invoice_items_tax_id_fkey FOREIGN KEY (tax_id) REFERENCES public.taxes(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.invoice_refunds
    ADD CONSTRAINT invoice_refunds_bank_account_id_fkey FOREIGN KEY (bank_account_id) REFERENCES public.bank_accounts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.invoice_refunds
    ADD CONSTRAINT invoice_refunds_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.invoice_refunds
    ADD CONSTRAINT invoice_refunds_invoice_id_fkey FOREIGN KEY (invoice_id) REFERENCES public.invoices(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.invoices
    ADD CONSTRAINT invoices_authorized_by_id_fkey FOREIGN KEY (authorized_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.invoices
    ADD CONSTRAINT invoices_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.invoices
    ADD CONSTRAINT invoices_customer_contact_id_fkey FOREIGN KEY (customer_contact_id) REFERENCES public.contacts(id);

ALTER TABLE ONLY public.invoices
    ADD CONSTRAINT invoices_customer_org_id_fkey FOREIGN KEY (customer_org_id) REFERENCES public.organizations(id);

ALTER TABLE ONLY public.invoices
    ADD CONSTRAINT invoices_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.invoices
    ADD CONSTRAINT invoices_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.invoices
    ADD CONSTRAINT invoices_quotation_id_fkey FOREIGN KEY (quotation_id) REFERENCES public.quotations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.issue_activity
    ADD CONSTRAINT issue_activity_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.issue_comments
    ADD CONSTRAINT issue_comments_author_id_fkey FOREIGN KEY (author_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.job_openings
    ADD CONSTRAINT job_openings_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.job_openings
    ADD CONSTRAINT job_openings_hiring_request_id_fkey FOREIGN KEY (hiring_request_id) REFERENCES public.hiring_requests(id);

ALTER TABLE ONLY public.journal_entries
    ADD CONSTRAINT journal_entries_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.journal_entries
    ADD CONSTRAINT journal_entries_fiscal_period_id_fkey FOREIGN KEY (fiscal_period_id) REFERENCES public.fiscal_periods(id);

ALTER TABLE ONLY public.journal_entries
    ADD CONSTRAINT journal_entries_posted_by_id_fkey FOREIGN KEY (posted_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.journal_entries
    ADD CONSTRAINT journal_entries_reversal_entry_id_fkey FOREIGN KEY (reversal_entry_id) REFERENCES public.journal_entries(id);

ALTER TABLE ONLY public.journal_entries
    ADD CONSTRAINT journal_entries_reversed_by_id_fkey FOREIGN KEY (reversed_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.journal_lines
    ADD CONSTRAINT journal_lines_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.accounts(id);

ALTER TABLE ONLY public.journal_lines
    ADD CONSTRAINT journal_lines_bank_statement_line_id_fkey FOREIGN KEY (bank_statement_line_id) REFERENCES public.bank_statement_lines(id);

ALTER TABLE ONLY public.journal_lines
    ADD CONSTRAINT journal_lines_entry_id_fkey FOREIGN KEY (entry_id) REFERENCES public.journal_entries(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.kb_article_revisions
    ADD CONSTRAINT kb_article_revisions_article_id_fkey FOREIGN KEY (article_id) REFERENCES public.kb_articles(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.kb_article_revisions
    ADD CONSTRAINT kb_article_revisions_changed_by_id_fkey FOREIGN KEY (changed_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.kb_articles
    ADD CONSTRAINT kb_articles_category_id_fkey FOREIGN KEY (category_id) REFERENCES public.kb_categories(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.kb_articles
    ADD CONSTRAINT kb_articles_owner_signed_by_id_fkey FOREIGN KEY (owner_signed_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.kb_articles
    ADD CONSTRAINT kb_articles_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.kb_articles
    ADD CONSTRAINT kb_articles_previous_version_id_fkey FOREIGN KEY (previous_version_id) REFERENCES public.kb_articles(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.kb_articles
    ADD CONSTRAINT kb_articles_published_by_id_fkey FOREIGN KEY (published_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.kb_articles
    ADD CONSTRAINT kb_articles_reviewed_by_id_fkey FOREIGN KEY (reviewed_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.kb_categories
    ADD CONSTRAINT kb_categories_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES public.kb_categories(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.kb_embeddings
    ADD CONSTRAINT kb_embeddings_article_id_fkey FOREIGN KEY (article_id) REFERENCES public.kb_articles(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.kiosk_devices
    ADD CONSTRAINT kiosk_devices_office_location_id_fkey FOREIGN KEY (office_location_id) REFERENCES public.office_locations(id);

ALTER TABLE ONLY public.kiosk_devices
    ADD CONSTRAINT kiosk_devices_registered_by_id_fkey FOREIGN KEY (registered_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.lead_attributions
    ADD CONSTRAINT lead_attributions_campaign_id_fkey FOREIGN KEY (campaign_id) REFERENCES public.marketing_campaigns(id);

ALTER TABLE ONLY public.lead_attributions
    ADD CONSTRAINT lead_attributions_content_item_id_fkey FOREIGN KEY (content_item_id) REFERENCES public.content_calendar(id);

ALTER TABLE ONLY public.lead_attributions
    ADD CONSTRAINT lead_attributions_lead_id_fkey FOREIGN KEY (lead_id) REFERENCES public.leads(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.leads
    ADD CONSTRAINT leads_assigned_to_id_fkey FOREIGN KEY (assigned_to_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.leads
    ADD CONSTRAINT leads_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.leads
    ADD CONSTRAINT leads_converted_to_deal_id_fkey FOREIGN KEY (converted_to_deal_id) REFERENCES public.deals(id);

ALTER TABLE ONLY public.leads
    ADD CONSTRAINT leads_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.leads
    ADD CONSTRAINT leads_organization_id_fkey FOREIGN KEY (organization_id) REFERENCES public.organizations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.leads
    ADD CONSTRAINT leads_partner_id_fkey FOREIGN KEY (partner_id) REFERENCES public.partner_accounts(id);

ALTER TABLE ONLY public.leads
    ADD CONSTRAINT leads_source_campaign_id_fkey FOREIGN KEY (source_campaign_id) REFERENCES public.marketing_campaigns(id);

ALTER TABLE ONLY public.leave_requests
    ADD CONSTRAINT leave_requests_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.leave_requests
    ADD CONSTRAINT leave_requests_employee_id_fkey FOREIGN KEY (employee_id) REFERENCES public.employees(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.leave_requests
    ADD CONSTRAINT leave_requests_submitted_by_id_fkey FOREIGN KEY (submitted_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.library_article_links
    ADD CONSTRAINT library_article_links_article_id_fkey FOREIGN KEY (article_id) REFERENCES public.kb_articles(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.library_article_links
    ADD CONSTRAINT library_article_links_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.library_documents(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.library_document_products
    ADD CONSTRAINT library_document_products_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.library_documents(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.library_document_products
    ADD CONSTRAINT library_document_products_product_id_fkey FOREIGN KEY (product_id) REFERENCES public.product_catalog(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.library_document_versions
    ADD CONSTRAINT library_document_versions_approved_by_fkey FOREIGN KEY (approved_by) REFERENCES public.users(id);

ALTER TABLE ONLY public.library_document_versions
    ADD CONSTRAINT library_document_versions_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.library_documents(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.library_document_versions
    ADD CONSTRAINT library_document_versions_uploaded_by_fkey FOREIGN KEY (uploaded_by) REFERENCES public.users(id);

ALTER TABLE ONLY public.library_documents
    ADD CONSTRAINT library_documents_category_id_fkey FOREIGN KEY (category_id) REFERENCES public.doc_categories(id);

ALTER TABLE ONLY public.library_documents
    ADD CONSTRAINT library_documents_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(id);

ALTER TABLE ONLY public.library_documents
    ADD CONSTRAINT library_documents_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.library_downloads
    ADD CONSTRAINT library_downloads_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.library_documents(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.library_downloads
    ADD CONSTRAINT library_downloads_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.library_downloads
    ADD CONSTRAINT library_downloads_version_id_fkey FOREIGN KEY (version_id) REFERENCES public.library_document_versions(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.mailing_list_contacts
    ADD CONSTRAINT mailing_list_contacts_list_id_fkey FOREIGN KEY (list_id) REFERENCES public.mailing_lists(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.mailing_lists
    ADD CONSTRAINT mailing_lists_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.maintenance_equipment
    ADD CONSTRAINT maintenance_equipment_asset_id_fkey FOREIGN KEY (asset_id) REFERENCES public.fixed_assets(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.maintenance_plans
    ADD CONSTRAINT maintenance_plans_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.maintenance_plans
    ADD CONSTRAINT maintenance_plans_equipment_id_fkey FOREIGN KEY (equipment_id) REFERENCES public.maintenance_equipment(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.maintenance_requests
    ADD CONSTRAINT maintenance_requests_equipment_id_fkey FOREIGN KEY (equipment_id) REFERENCES public.maintenance_equipment(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.maintenance_requests
    ADD CONSTRAINT maintenance_requests_reported_by_id_fkey FOREIGN KEY (reported_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.maintenance_requests
    ADD CONSTRAINT maintenance_requests_reviewed_by_id_fkey FOREIGN KEY (reviewed_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.maintenance_work_orders
    ADD CONSTRAINT maintenance_work_orders_assigned_to_id_fkey FOREIGN KEY (assigned_to_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.maintenance_work_orders
    ADD CONSTRAINT maintenance_work_orders_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.maintenance_work_orders
    ADD CONSTRAINT maintenance_work_orders_equipment_id_fkey FOREIGN KEY (equipment_id) REFERENCES public.maintenance_equipment(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.maintenance_work_orders
    ADD CONSTRAINT maintenance_work_orders_request_id_fkey FOREIGN KEY (request_id) REFERENCES public.maintenance_requests(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.maintenance_work_orders
    ADD CONSTRAINT maintenance_work_orders_vendor_id_fkey FOREIGN KEY (vendor_id) REFERENCES public.vendors(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.marketing_campaigns
    ADD CONSTRAINT marketing_campaigns_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.mcp_oauth_codes
    ADD CONSTRAINT mcp_oauth_codes_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.mcp_oauth_clients(client_id) ON DELETE CASCADE;

ALTER TABLE ONLY public.mcp_oauth_codes
    ADD CONSTRAINT mcp_oauth_codes_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.mdf_claims
    ADD CONSTRAINT mdf_claims_mdf_request_id_fkey FOREIGN KEY (mdf_request_id) REFERENCES public.mdf_requests(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.mdf_claims
    ADD CONSTRAINT mdf_claims_reviewed_by_id_fkey FOREIGN KEY (reviewed_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.mdf_claims
    ADD CONSTRAINT mdf_claims_submitted_by_id_fkey FOREIGN KEY (submitted_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.mdf_requests
    ADD CONSTRAINT mdf_requests_partner_id_fkey FOREIGN KEY (partner_id) REFERENCES public.partner_accounts(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.mdf_requests
    ADD CONSTRAINT mdf_requests_reviewed_by_id_fkey FOREIGN KEY (reviewed_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.mdf_requests
    ADD CONSTRAINT mdf_requests_submitted_by_id_fkey FOREIGN KEY (submitted_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.memos
    ADD CONSTRAINT memos_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.channels(id);

ALTER TABLE ONLY public.memos
    ADD CONSTRAINT memos_recipient_id_fkey FOREIGN KEY (recipient_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.memos
    ADD CONSTRAINT memos_sender_id_fkey FOREIGN KEY (sender_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.message_reactions
    ADD CONSTRAINT message_reactions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.nary_cadences
    ADD CONSTRAINT nary_cadences_card_id_fkey FOREIGN KEY (card_id) REFERENCES public.agent_pending_actions(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_cadences
    ADD CONSTRAINT nary_cadences_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_cadences
    ADD CONSTRAINT nary_cadences_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_cadences
    ADD CONSTRAINT nary_cadences_quotation_id_fkey FOREIGN KEY (quotation_id) REFERENCES public.quotations(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.nary_owner_topics
    ADD CONSTRAINT nary_owner_topics_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.ai_conversations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_cost_sheets
    ADD CONSTRAINT nary_cost_sheets_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_cost_sheets
    ADD CONSTRAINT nary_cost_sheets_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.sales_documents(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.nary_cost_sheets
    ADD CONSTRAINT nary_cost_sheets_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.agent_runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_expected_arrivals
    ADD CONSTRAINT nary_expected_arrivals_lead_id_fkey FOREIGN KEY (lead_id) REFERENCES public.leads(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.nary_file_links
    ADD CONSTRAINT nary_file_links_file_id_fkey FOREIGN KEY (file_id) REFERENCES public.nary_files(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.nary_identity_links
    ADD CONSTRAINT nary_identity_links_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_identity_links
    ADD CONSTRAINT nary_identity_links_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_identity_links
    ADD CONSTRAINT nary_identity_links_decided_by_id_fkey FOREIGN KEY (decided_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_identity_links
    ADD CONSTRAINT nary_identity_links_lead_id_fkey FOREIGN KEY (lead_id) REFERENCES public.leads(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_insights
    ADD CONSTRAINT nary_insights_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.nary_insights
    ADD CONSTRAINT nary_insights_lead_id_fkey FOREIGN KEY (lead_id) REFERENCES public.leads(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.nary_insights
    ADD CONSTRAINT nary_insights_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.agent_runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_meeting_files
    ADD CONSTRAINT nary_meeting_files_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_meeting_files
    ADD CONSTRAINT nary_meeting_files_meeting_id_fkey FOREIGN KEY (meeting_id) REFERENCES public.nary_meetings(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_meeting_files
    ADD CONSTRAINT nary_meeting_files_requested_by_id_fkey FOREIGN KEY (requested_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_meetings
    ADD CONSTRAINT nary_meetings_booked_by_id_fkey FOREIGN KEY (booked_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_meetings
    ADD CONSTRAINT nary_meetings_card_id_fkey FOREIGN KEY (card_id) REFERENCES public.agent_pending_actions(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_meetings
    ADD CONSTRAINT nary_meetings_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_meetings
    ADD CONSTRAINT nary_meetings_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_meetings
    ADD CONSTRAINT nary_meetings_lead_id_fkey FOREIGN KEY (lead_id) REFERENCES public.leads(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_memory
    ADD CONSTRAINT nary_memory_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_memory
    ADD CONSTRAINT nary_memory_author_id_fkey FOREIGN KEY (author_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_outbound_log
    ADD CONSTRAINT nary_outbound_log_card_id_fkey FOREIGN KEY (card_id) REFERENCES public.agent_pending_actions(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_outbound_log
    ADD CONSTRAINT nary_outbound_log_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_outbound_log
    ADD CONSTRAINT nary_outbound_log_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_rate_card
    ADD CONSTRAINT nary_rate_card_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_reply_alerts
    ADD CONSTRAINT nary_reply_alerts_card_id_fkey FOREIGN KEY (card_id) REFERENCES public.agent_pending_actions(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_reply_alerts
    ADD CONSTRAINT nary_reply_alerts_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_reply_alerts
    ADD CONSTRAINT nary_reply_alerts_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_reply_alerts
    ADD CONSTRAINT nary_reply_alerts_lead_id_fkey FOREIGN KEY (lead_id) REFERENCES public.leads(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_reply_alerts
    ADD CONSTRAINT nary_reply_alerts_topic_id_fkey FOREIGN KEY (topic_id) REFERENCES public.nary_owner_topics(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_reply_jobs
    ADD CONSTRAINT nary_reply_jobs_card_id_fkey FOREIGN KEY (card_id) REFERENCES public.agent_pending_actions(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_reply_jobs
    ADD CONSTRAINT nary_reply_jobs_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.agent_runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_role_assignments
    ADD CONSTRAINT nary_role_assignments_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_scores
    ADD CONSTRAINT nary_scores_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.nary_scores
    ADD CONSTRAINT nary_scores_lead_id_fkey FOREIGN KEY (lead_id) REFERENCES public.leads(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.nary_scores
    ADD CONSTRAINT nary_scores_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.agent_runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_triage_verdicts
    ADD CONSTRAINT nary_triage_verdicts_corrected_by_id_fkey FOREIGN KEY (corrected_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.nary_triage_verdicts
    ADD CONSTRAINT nary_triage_verdicts_lead_id_fkey FOREIGN KEY (lead_id) REFERENCES public.leads(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.note_templates
    ADD CONSTRAINT note_templates_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.offer_letters
    ADD CONSTRAINT offer_letters_applicant_id_fkey FOREIGN KEY (applicant_id) REFERENCES public.applicants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.offer_letters
    ADD CONSTRAINT offer_letters_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.offer_letters
    ADD CONSTRAINT offer_letters_hiring_request_id_fkey FOREIGN KEY (hiring_request_id) REFERENCES public.hiring_requests(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.offer_letters
    ADD CONSTRAINT offer_letters_job_opening_id_fkey FOREIGN KEY (job_opening_id) REFERENCES public.job_openings(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.offer_letters
    ADD CONSTRAINT offer_letters_reports_to_employee_id_fkey FOREIGN KEY (reports_to_employee_id) REFERENCES public.employees(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.organizations
    ADD CONSTRAINT organizations_assigned_to_id_fkey FOREIGN KEY (assigned_to_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.organizations
    ADD CONSTRAINT organizations_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.partner_accounts
    ADD CONSTRAINT partner_accounts_assigned_am_id_fkey FOREIGN KEY (assigned_am_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.partner_accounts
    ADD CONSTRAINT partner_accounts_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.partner_accounts
    ADD CONSTRAINT partner_accounts_organization_id_fkey FOREIGN KEY (organization_id) REFERENCES public.organizations(id);

ALTER TABLE ONLY public.partner_accounts
    ADD CONSTRAINT partner_accounts_primary_contact_id_fkey FOREIGN KEY (primary_contact_id) REFERENCES public.contacts(id);

ALTER TABLE ONLY public.partner_certifications
    ADD CONSTRAINT partner_certifications_module_id_fkey FOREIGN KEY (module_id) REFERENCES public.training_modules(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.partner_certifications
    ADD CONSTRAINT partner_certifications_partner_user_id_fkey FOREIGN KEY (partner_user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.partner_commissions
    ADD CONSTRAINT partner_commissions_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.partner_commissions
    ADD CONSTRAINT partner_commissions_deal_registration_id_fkey FOREIGN KEY (deal_registration_id) REFERENCES public.partner_deal_registrations(id);

ALTER TABLE ONLY public.partner_commissions
    ADD CONSTRAINT partner_commissions_invoice_id_fkey FOREIGN KEY (invoice_id) REFERENCES public.invoices(id);

ALTER TABLE ONLY public.partner_commissions
    ADD CONSTRAINT partner_commissions_partner_id_fkey FOREIGN KEY (partner_id) REFERENCES public.partner_accounts(id);

ALTER TABLE ONLY public.partner_deal_registrations
    ADD CONSTRAINT partner_deal_registrations_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.partner_deal_registrations
    ADD CONSTRAINT partner_deal_registrations_conflicting_registration_id_fkey FOREIGN KEY (conflicting_registration_id) REFERENCES public.partner_deal_registrations(id);

ALTER TABLE ONLY public.partner_deal_registrations
    ADD CONSTRAINT partner_deal_registrations_customer_org_id_fkey FOREIGN KEY (customer_org_id) REFERENCES public.organizations(id);

ALTER TABLE ONLY public.partner_deal_registrations
    ADD CONSTRAINT partner_deal_registrations_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id);

ALTER TABLE ONLY public.partner_deal_registrations
    ADD CONSTRAINT partner_deal_registrations_partner_id_fkey FOREIGN KEY (partner_id) REFERENCES public.partner_accounts(id);

ALTER TABLE ONLY public.partner_leads
    ADD CONSTRAINT partner_leads_assigned_by_id_fkey FOREIGN KEY (assigned_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.partner_leads
    ADD CONSTRAINT partner_leads_converted_deal_id_fkey FOREIGN KEY (converted_deal_id) REFERENCES public.partner_deal_registrations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.partner_leads
    ADD CONSTRAINT partner_leads_partner_id_fkey FOREIGN KEY (partner_id) REFERENCES public.partner_accounts(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.partner_module_progress
    ADD CONSTRAINT partner_module_progress_module_id_fkey FOREIGN KEY (module_id) REFERENCES public.training_modules(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.partner_module_progress
    ADD CONSTRAINT partner_module_progress_partner_user_id_fkey FOREIGN KEY (partner_user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.partner_resources
    ADD CONSTRAINT partner_resources_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.payment_schedules
    ADD CONSTRAINT payment_schedules_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.payment_schedules
    ADD CONSTRAINT payment_schedules_invoice_id_fkey FOREIGN KEY (invoice_id) REFERENCES public.invoices(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.payment_schedules
    ADD CONSTRAINT payment_schedules_milestone_id_fkey FOREIGN KEY (milestone_id) REFERENCES public.project_milestones(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.payment_schedules
    ADD CONSTRAINT payment_schedules_proforma_invoice_id_fkey FOREIGN KEY (proforma_invoice_id) REFERENCES public.invoices(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.payment_schedules
    ADD CONSTRAINT payment_schedules_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.payment_schedules
    ADD CONSTRAINT payment_schedules_quotation_id_fkey FOREIGN KEY (quotation_id) REFERENCES public.quotations(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.payments
    ADD CONSTRAINT payments_bank_account_id_fkey FOREIGN KEY (bank_account_id) REFERENCES public.bank_accounts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.payments
    ADD CONSTRAINT payments_confirmed_by_id_fkey FOREIGN KEY (confirmed_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.payments
    ADD CONSTRAINT payments_invoice_id_fkey FOREIGN KEY (invoice_id) REFERENCES public.invoices(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.payroll_lines
    ADD CONSTRAINT payroll_lines_employee_id_fkey FOREIGN KEY (employee_id) REFERENCES public.employees(id);

ALTER TABLE ONLY public.payroll_lines
    ADD CONSTRAINT payroll_lines_payroll_run_id_fkey FOREIGN KEY (payroll_run_id) REFERENCES public.payroll_runs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.payroll_runs
    ADD CONSTRAINT payroll_runs_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.payroll_runs
    ADD CONSTRAINT payroll_runs_bank_account_id_fkey FOREIGN KEY (bank_account_id) REFERENCES public.bank_accounts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.payroll_runs
    ADD CONSTRAINT payroll_runs_calculated_by_id_fkey FOREIGN KEY (calculated_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.personal_access_tokens
    ADD CONSTRAINT personal_access_tokens_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.mcp_oauth_clients(client_id) ON DELETE SET NULL;

ALTER TABLE ONLY public.personal_access_tokens
    ADD CONSTRAINT personal_access_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.portal_registration_requests
    ADD CONSTRAINT portal_registration_requests_created_user_id_fkey FOREIGN KEY (created_user_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.portal_registration_requests
    ADD CONSTRAINT portal_registration_requests_reviewed_by_id_fkey FOREIGN KEY (reviewed_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.product_catalog
    ADD CONSTRAINT product_catalog_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.product_catalog
    ADD CONSTRAINT product_catalog_hardware_product_id_fkey FOREIGN KEY (hardware_product_id) REFERENCES public.hardware_products(id);

ALTER TABLE ONLY public.product_catalog
    ADD CONSTRAINT product_catalog_product_line_id_fkey FOREIGN KEY (product_line_id) REFERENCES public.product_lines(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.production_orders
    ADD CONSTRAINT production_orders_assigned_to_id_fkey FOREIGN KEY (assigned_to_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.production_orders
    ADD CONSTRAINT production_orders_bom_id_fkey FOREIGN KEY (bom_id) REFERENCES public.boms(id);

ALTER TABLE ONLY public.production_orders
    ADD CONSTRAINT production_orders_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.production_orders
    ADD CONSTRAINT production_orders_customer_org_id_fkey FOREIGN KEY (customer_org_id) REFERENCES public.organizations(id);

ALTER TABLE ONLY public.production_orders
    ADD CONSTRAINT production_orders_invoice_id_fkey FOREIGN KEY (invoice_id) REFERENCES public.invoices(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.production_orders
    ADD CONSTRAINT production_orders_product_id_fkey FOREIGN KEY (product_id) REFERENCES public.hardware_products(id);

ALTER TABLE ONLY public.production_orders
    ADD CONSTRAINT production_orders_subcontractor_vendor_id_fkey FOREIGN KEY (subcontractor_vendor_id) REFERENCES public.vendors(id);

ALTER TABLE ONLY public.production_orders
    ADD CONSTRAINT production_orders_subcontractor_work_order_id_fkey FOREIGN KEY (subcontractor_work_order_id) REFERENCES public.work_orders(id);

ALTER TABLE ONLY public.production_units
    ADD CONSTRAINT production_units_bom_id_fkey FOREIGN KEY (bom_id) REFERENCES public.boms(id);

ALTER TABLE ONLY public.production_units
    ADD CONSTRAINT production_units_invoice_id_fkey FOREIGN KEY (invoice_id) REFERENCES public.invoices(id);

ALTER TABLE ONLY public.production_units
    ADD CONSTRAINT production_units_produced_by_id_fkey FOREIGN KEY (produced_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.production_units
    ADD CONSTRAINT production_units_product_id_fkey FOREIGN KEY (product_id) REFERENCES public.hardware_products(id);

ALTER TABLE ONLY public.production_units
    ADD CONSTRAINT production_units_production_order_id_fkey FOREIGN KEY (production_order_id) REFERENCES public.production_orders(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.project_members
    ADD CONSTRAINT project_members_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.project_members
    ADD CONSTRAINT project_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.project_milestones
    ADD CONSTRAINT project_milestones_invoice_id_fkey FOREIGN KEY (invoice_id) REFERENCES public.invoices(id);

ALTER TABLE ONLY public.project_milestones
    ADD CONSTRAINT project_milestones_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.project_phases
    ADD CONSTRAINT project_phases_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.project_tasks
    ADD CONSTRAINT project_tasks_assigned_to_id_fkey FOREIGN KEY (assigned_to_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.project_tasks
    ADD CONSTRAINT project_tasks_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.project_tasks
    ADD CONSTRAINT project_tasks_epic_id_fkey FOREIGN KEY (epic_id) REFERENCES public.epics(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.project_tasks
    ADD CONSTRAINT project_tasks_parent_task_id_fkey FOREIGN KEY (parent_task_id) REFERENCES public.project_tasks(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.project_tasks
    ADD CONSTRAINT project_tasks_phase_id_fkey FOREIGN KEY (phase_id) REFERENCES public.project_phases(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.project_tasks
    ADD CONSTRAINT project_tasks_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_contract_id_fkey FOREIGN KEY (contract_id) REFERENCES public.contracts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_customer_contact_id_fkey FOREIGN KEY (customer_contact_id) REFERENCES public.contacts(id);

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_customer_org_id_fkey FOREIGN KEY (customer_org_id) REFERENCES public.organizations(id);

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_pm_id_fkey FOREIGN KEY (pm_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_tech_lead_id_fkey FOREIGN KEY (tech_lead_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.purchase_order_items
    ADD CONSTRAINT purchase_order_items_po_id_fkey FOREIGN KEY (po_id) REFERENCES public.purchase_orders(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.purchase_order_items
    ADD CONSTRAINT purchase_order_items_rfq_line_id_fkey FOREIGN KEY (rfq_line_id) REFERENCES public.purchase_rfq_lines(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.purchase_orders
    ADD CONSTRAINT purchase_orders_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.purchase_orders
    ADD CONSTRAINT purchase_orders_contractor_request_id_fkey FOREIGN KEY (contractor_request_id) REFERENCES public.contractor_requests(id);

ALTER TABLE ONLY public.purchase_orders
    ADD CONSTRAINT purchase_orders_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.purchase_orders
    ADD CONSTRAINT purchase_orders_requested_by_id_fkey FOREIGN KEY (requested_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.purchase_orders
    ADD CONSTRAINT purchase_orders_rfq_id_fkey FOREIGN KEY (rfq_id) REFERENCES public.purchase_rfqs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.purchase_orders
    ADD CONSTRAINT purchase_orders_vendor_id_fkey FOREIGN KEY (vendor_id) REFERENCES public.vendors(id);

ALTER TABLE ONLY public.purchase_rfq_line_responses
    ADD CONSTRAINT purchase_rfq_line_responses_rfq_id_fkey FOREIGN KEY (rfq_id) REFERENCES public.purchase_rfqs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.purchase_rfq_line_responses
    ADD CONSTRAINT purchase_rfq_line_responses_rfq_line_id_fkey FOREIGN KEY (rfq_line_id) REFERENCES public.purchase_rfq_lines(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.purchase_rfq_line_responses
    ADD CONSTRAINT purchase_rfq_line_responses_vendor_id_fkey FOREIGN KEY (vendor_id) REFERENCES public.vendors(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.purchase_rfq_lines
    ADD CONSTRAINT purchase_rfq_lines_rfq_id_fkey FOREIGN KEY (rfq_id) REFERENCES public.purchase_rfqs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.purchase_rfq_vendor_invites
    ADD CONSTRAINT purchase_rfq_vendor_invites_rfq_id_fkey FOREIGN KEY (rfq_id) REFERENCES public.purchase_rfqs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.purchase_rfq_vendor_invites
    ADD CONSTRAINT purchase_rfq_vendor_invites_vendor_id_fkey FOREIGN KEY (vendor_id) REFERENCES public.vendors(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.purchase_rfqs
    ADD CONSTRAINT purchase_rfqs_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.qc_records
    ADD CONSTRAINT qc_records_checked_by_id_fkey FOREIGN KEY (checked_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.qc_records
    ADD CONSTRAINT qc_records_unit_id_fkey FOREIGN KEY (unit_id) REFERENCES public.production_units(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.quotation_items
    ADD CONSTRAINT quotation_items_parent_item_id_fkey FOREIGN KEY (parent_item_id) REFERENCES public.quotation_items(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.quotation_items
    ADD CONSTRAINT quotation_items_product_id_fkey FOREIGN KEY (product_id) REFERENCES public.product_catalog(id);

ALTER TABLE ONLY public.quotation_items
    ADD CONSTRAINT quotation_items_quotation_id_fkey FOREIGN KEY (quotation_id) REFERENCES public.quotations(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.quotation_items
    ADD CONSTRAINT quotation_items_tax_id_fkey FOREIGN KEY (tax_id) REFERENCES public.taxes(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.quotations
    ADD CONSTRAINT quotations_approved_by_id_fkey FOREIGN KEY (approved_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.quotations
    ADD CONSTRAINT quotations_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.quotations
    ADD CONSTRAINT quotations_customer_contact_id_fkey FOREIGN KEY (customer_contact_id) REFERENCES public.contacts(id);

ALTER TABLE ONLY public.quotations
    ADD CONSTRAINT quotations_customer_org_id_fkey FOREIGN KEY (customer_org_id) REFERENCES public.organizations(id);

ALTER TABLE ONLY public.quotations
    ADD CONSTRAINT quotations_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.quotations
    ADD CONSTRAINT quotations_pricing_override_by_id_fkey FOREIGN KEY (pricing_override_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.rma_cases
    ADD CONSTRAINT rma_cases_customer_org_id_fkey FOREIGN KEY (customer_org_id) REFERENCES public.organizations(id);

ALTER TABLE ONLY public.rma_cases
    ADD CONSTRAINT rma_cases_handled_by_id_fkey FOREIGN KEY (handled_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.rma_cases
    ADD CONSTRAINT rma_cases_replacement_unit_id_fkey FOREIGN KEY (replacement_unit_id) REFERENCES public.production_units(id);

ALTER TABLE ONLY public.rma_cases
    ADD CONSTRAINT rma_cases_unit_id_fkey FOREIGN KEY (unit_id) REFERENCES public.production_units(id);

ALTER TABLE ONLY public.rma_cases
    ADD CONSTRAINT rma_cases_warranty_id_fkey FOREIGN KEY (warranty_id) REFERENCES public.warranty_records(id);

ALTER TABLE ONLY public.rnd_backlog_items
    ADD CONSTRAINT rnd_backlog_items_assignee_id_fkey FOREIGN KEY (assignee_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.rnd_backlog_items
    ADD CONSTRAINT rnd_backlog_items_epic_id_fkey FOREIGN KEY (epic_id) REFERENCES public.epics(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.rnd_backlog_items
    ADD CONSTRAINT rnd_backlog_items_gitlab_repository_id_fkey FOREIGN KEY (gitlab_repository_id) REFERENCES public.gitlab_repositories(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.rnd_backlog_items
    ADD CONSTRAINT rnd_backlog_items_reporter_id_fkey FOREIGN KEY (reporter_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.rnd_backlog_items
    ADD CONSTRAINT rnd_backlog_items_rnd_project_id_fkey FOREIGN KEY (rnd_project_id) REFERENCES public.rnd_projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.rnd_backlog_items
    ADD CONSTRAINT rnd_backlog_items_sprint_id_fkey FOREIGN KEY (sprint_id) REFERENCES public.rnd_sprints(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.rnd_projects
    ADD CONSTRAINT rnd_projects_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.rnd_projects
    ADD CONSTRAINT rnd_projects_hardware_product_id_fkey FOREIGN KEY (hardware_product_id) REFERENCES public.hardware_products(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.rnd_projects
    ADD CONSTRAINT rnd_projects_lead_engineer_id_fkey FOREIGN KEY (lead_engineer_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.rnd_releases
    ADD CONSTRAINT rnd_releases_rnd_project_id_fkey FOREIGN KEY (rnd_project_id) REFERENCES public.rnd_projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.rnd_sprints
    ADD CONSTRAINT rnd_sprints_rnd_project_id_fkey FOREIGN KEY (rnd_project_id) REFERENCES public.rnd_projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.role_permissions
    ADD CONSTRAINT role_permissions_permission_id_fkey FOREIGN KEY (permission_id) REFERENCES public.permissions(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.role_permissions
    ADD CONSTRAINT role_permissions_role_id_fkey FOREIGN KEY (role_id) REFERENCES public.roles(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sales_document_assets
    ADD CONSTRAINT sales_document_assets_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.sales_document_assets
    ADD CONSTRAINT sales_document_assets_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.sales_documents(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sales_document_blocks
    ADD CONSTRAINT sales_document_blocks_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.sales_documents(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sales_document_jobs
    ADD CONSTRAINT sales_document_jobs_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.sales_document_jobs
    ADD CONSTRAINT sales_document_jobs_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.sales_documents(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sales_document_quotations
    ADD CONSTRAINT sales_document_quotations_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.sales_documents(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sales_document_quotations
    ADD CONSTRAINT sales_document_quotations_quotation_id_fkey FOREIGN KEY (quotation_id) REFERENCES public.quotations(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sales_documents
    ADD CONSTRAINT sales_documents_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.sales_documents
    ADD CONSTRAINT sales_documents_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.sales_documents
    ADD CONSTRAINT sales_documents_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.sales_documents
    ADD CONSTRAINT sales_documents_organization_id_fkey FOREIGN KEY (organization_id) REFERENCES public.organizations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.scheduled_messages
    ADD CONSTRAINT scheduled_messages_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.scheduled_messages
    ADD CONSTRAINT scheduled_messages_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sign_audit_logs
    ADD CONSTRAINT sign_audit_logs_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.sign_documents(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sign_audit_logs
    ADD CONSTRAINT sign_audit_logs_signer_id_fkey FOREIGN KEY (signer_id) REFERENCES public.sign_signers(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.sign_documents
    ADD CONSTRAINT sign_documents_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.sign_fields
    ADD CONSTRAINT sign_fields_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.sign_documents(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sign_fields
    ADD CONSTRAINT sign_fields_signer_id_fkey FOREIGN KEY (signer_id) REFERENCES public.sign_signers(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sign_signers
    ADD CONSTRAINT sign_signers_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.sign_documents(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sign_signers
    ADD CONSTRAINT sign_signers_partner_account_id_fkey FOREIGN KEY (partner_account_id) REFERENCES public.partner_accounts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.sign_signers
    ADD CONSTRAINT sign_signers_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.staff_whatsapp_links
    ADD CONSTRAINT staff_whatsapp_links_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.staff_whatsapp_links
    ADD CONSTRAINT staff_whatsapp_links_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.stock_locations
    ADD CONSTRAINT stock_locations_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES public.stock_locations(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.stock_locations
    ADD CONSTRAINT stock_locations_warehouse_id_fkey FOREIGN KEY (warehouse_id) REFERENCES public.warehouses(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.stock_moves
    ADD CONSTRAINT stock_moves_from_location_id_fkey FOREIGN KEY (from_location_id) REFERENCES public.stock_locations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.stock_moves
    ADD CONSTRAINT stock_moves_moved_by_id_fkey FOREIGN KEY (moved_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.stock_moves
    ADD CONSTRAINT stock_moves_to_location_id_fkey FOREIGN KEY (to_location_id) REFERENCES public.stock_locations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.system_settings
    ADD CONSTRAINT system_settings_updated_by_id_fkey FOREIGN KEY (updated_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.tax_configs
    ADD CONSTRAINT tax_configs_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.tax_configs
    ADD CONSTRAINT tax_configs_updated_by_id_fkey FOREIGN KEY (updated_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.tax_rules
    ADD CONSTRAINT tax_rules_config_id_fkey FOREIGN KEY (config_id) REFERENCES public.tax_configs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.tax_rules
    ADD CONSTRAINT tax_rules_updated_by_id_fkey FOREIGN KEY (updated_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.taxes
    ADD CONSTRAINT taxes_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.taxes
    ADD CONSTRAINT taxes_tax_payable_account_id_fkey FOREIGN KEY (tax_payable_account_id) REFERENCES public.accounts(id);

ALTER TABLE ONLY public.taxes
    ADD CONSTRAINT taxes_tax_receivable_account_id_fkey FOREIGN KEY (tax_receivable_account_id) REFERENCES public.accounts(id);

ALTER TABLE ONLY public.taxes
    ADD CONSTRAINT taxes_updated_by_id_fkey FOREIGN KEY (updated_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.termination_requests
    ADD CONSTRAINT termination_requests_confirmed_by_id_fkey FOREIGN KEY (confirmed_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.termination_requests
    ADD CONSTRAINT termination_requests_employee_id_fkey FOREIGN KEY (employee_id) REFERENCES public.employees(id);

ALTER TABLE ONLY public.termination_requests
    ADD CONSTRAINT termination_requests_initiated_by_id_fkey FOREIGN KEY (initiated_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.time_logs
    ADD CONSTRAINT time_logs_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.time_logs
    ADD CONSTRAINT time_logs_task_id_fkey FOREIGN KEY (task_id) REFERENCES public.project_tasks(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.time_logs
    ADD CONSTRAINT time_logs_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.training_chapters
    ADD CONSTRAINT training_chapters_module_id_fkey FOREIGN KEY (module_id) REFERENCES public.training_modules(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.training_modules
    ADD CONSTRAINT training_modules_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.training_quiz_questions
    ADD CONSTRAINT training_quiz_questions_chapter_id_fkey FOREIGN KEY (chapter_id) REFERENCES public.training_chapters(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.upload_link_files
    ADD CONSTRAINT upload_link_files_link_id_fkey FOREIGN KEY (link_id) REFERENCES public.upload_links(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.upload_links
    ADD CONSTRAINT upload_links_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.user_permissions
    ADD CONSTRAINT user_permissions_permission_id_fkey FOREIGN KEY (permission_id) REFERENCES public.permissions(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_permissions
    ADD CONSTRAINT user_permissions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_roles
    ADD CONSTRAINT user_roles_role_id_fkey FOREIGN KEY (role_id) REFERENCES public.roles(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_roles
    ADD CONSTRAINT user_roles_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_totp_failures
    ADD CONSTRAINT user_totp_failures_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_totp_recovery_codes
    ADD CONSTRAINT user_totp_recovery_codes_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_totp
    ADD CONSTRAINT user_totp_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_partner_account_id_fkey FOREIGN KEY (partner_account_id) REFERENCES public.partner_accounts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_vendor_id_fkey FOREIGN KEY (vendor_id) REFERENCES public.vendors(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.vendor_documents
    ADD CONSTRAINT vendor_documents_vendor_id_fkey FOREIGN KEY (vendor_id) REFERENCES public.vendors(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.vendor_documents
    ADD CONSTRAINT vendor_documents_verified_by_id_fkey FOREIGN KEY (verified_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.vendor_invoices
    ADD CONSTRAINT vendor_invoices_bank_account_id_fkey FOREIGN KEY (bank_account_id) REFERENCES public.bank_accounts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.vendor_invoices
    ADD CONSTRAINT vendor_invoices_purchase_order_id_fkey FOREIGN KEY (purchase_order_id) REFERENCES public.purchase_orders(id);

ALTER TABLE ONLY public.vendor_invoices
    ADD CONSTRAINT vendor_invoices_reviewed_by_id_fkey FOREIGN KEY (reviewed_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.vendor_invoices
    ADD CONSTRAINT vendor_invoices_vendor_id_fkey FOREIGN KEY (vendor_id) REFERENCES public.vendors(id);

ALTER TABLE ONLY public.vendor_invoices
    ADD CONSTRAINT vendor_invoices_work_order_id_fkey FOREIGN KEY (work_order_id) REFERENCES public.work_orders(id);

ALTER TABLE ONLY public.vendors
    ADD CONSTRAINT vendors_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.vendors
    ADD CONSTRAINT vendors_organization_id_fkey FOREIGN KEY (organization_id) REFERENCES public.organizations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.video_rooms
    ADD CONSTRAINT video_rooms_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.video_rooms
    ADD CONSTRAINT video_rooms_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id);

ALTER TABLE ONLY public.video_rooms
    ADD CONSTRAINT video_rooms_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id);

ALTER TABLE ONLY public.wa_chats
    ADD CONSTRAINT wa_chats_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.wa_chats
    ADD CONSTRAINT wa_chats_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.wa_chats
    ADD CONSTRAINT wa_chats_excluded_by_id_fkey FOREIGN KEY (excluded_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.wa_chats
    ADD CONSTRAINT wa_chats_lead_id_fkey FOREIGN KEY (lead_id) REFERENCES public.leads(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.wa_chats
    ADD CONSTRAINT wa_chats_staff_user_id_fkey FOREIGN KEY (staff_user_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.wa_contact_links
    ADD CONSTRAINT wa_contact_links_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.wa_hidden_chats
    ADD CONSTRAINT wa_hidden_chats_hidden_by_id_fkey FOREIGN KEY (hidden_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.wa_import_chats
    ADD CONSTRAINT wa_import_chats_decided_by_id_fkey FOREIGN KEY (decided_by_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.wa_messages
    ADD CONSTRAINT wa_messages_activity_id_fkey FOREIGN KEY (activity_id) REFERENCES public.activities(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.wa_messages
    ADD CONSTRAINT wa_messages_contact_id_fkey FOREIGN KEY (contact_id) REFERENCES public.contacts(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.wa_messages
    ADD CONSTRAINT wa_messages_deal_id_fkey FOREIGN KEY (deal_id) REFERENCES public.deals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.wa_messages
    ADD CONSTRAINT wa_messages_lead_id_fkey FOREIGN KEY (lead_id) REFERENCES public.leads(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.wa_sent_messages
    ADD CONSTRAINT wa_sent_messages_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.warranty_records
    ADD CONSTRAINT warranty_records_customer_org_id_fkey FOREIGN KEY (customer_org_id) REFERENCES public.organizations(id);

ALTER TABLE ONLY public.warranty_records
    ADD CONSTRAINT warranty_records_invoice_id_fkey FOREIGN KEY (invoice_id) REFERENCES public.invoices(id);

ALTER TABLE ONLY public.warranty_records
    ADD CONSTRAINT warranty_records_unit_id_fkey FOREIGN KEY (unit_id) REFERENCES public.production_units(id);

ALTER TABLE ONLY public.work_order_timesheets
    ADD CONSTRAINT work_order_timesheets_reviewed_by_id_fkey FOREIGN KEY (reviewed_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.work_order_timesheets
    ADD CONSTRAINT work_order_timesheets_vendor_user_id_fkey FOREIGN KEY (vendor_user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.work_order_timesheets
    ADD CONSTRAINT work_order_timesheets_work_order_id_fkey FOREIGN KEY (work_order_id) REFERENCES public.work_orders(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.work_orders
    ADD CONSTRAINT work_orders_completed_confirmed_by_id_fkey FOREIGN KEY (completed_confirmed_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.work_orders
    ADD CONSTRAINT work_orders_contractor_request_id_fkey FOREIGN KEY (contractor_request_id) REFERENCES public.contractor_requests(id);

ALTER TABLE ONLY public.work_orders
    ADD CONSTRAINT work_orders_created_by_id_fkey FOREIGN KEY (created_by_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.work_orders
    ADD CONSTRAINT work_orders_production_order_id_fkey FOREIGN KEY (production_order_id) REFERENCES public.production_orders(id);

ALTER TABLE ONLY public.work_orders
    ADD CONSTRAINT work_orders_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id);

ALTER TABLE ONLY public.work_orders
    ADD CONSTRAINT work_orders_vendor_id_fkey FOREIGN KEY (vendor_id) REFERENCES public.vendors(id);

-- Default data

INSERT INTO public.approval_policies (entity_type, name, description, sop_reference, is_active, version, preconditions, signature_flow, signer_role, sign_deadline_days, max_extension_days, qr_enabled, view_scope, allow_download, requires_login, token_ttl_days, sla_hours, whatsapp_escalate_above_base, updated_by_id, id, created_at, updated_at) VALUES ('library_document_version', 'Document version approval', 'A director approves each new version of a document that customers or partners receive before it goes live.', 'Document library', true, 1, '[]', 'none', NULL, NULL, NULL, false, 'metadata_document', false, false, 365, 72, NULL, NULL, '74b67bb7-6715-4e10-9507-8ec4d309f738', now(), now());

INSERT INTO public.approval_policy_steps (policy_id, sequence, name, approver_mode, role_slug, department, snapshot_user_key, user_id, condition, sla_hours, is_notify_only, allow_self_approval, id, created_at, updated_at) VALUES ('74b67bb7-6715-4e10-9507-8ec4d309f738', 1, 'Director approval', 'role', 'executive', NULL, NULL, NULL, NULL, 72, false, false, '13256359-1a8c-4d04-bcc8-c9bac8d727f7', now(), now());

INSERT INTO public.doc_categories (id, key, name, sort_order, visible_roles, created_at, updated_at) VALUES ('c20f5692-1cb5-4525-a53e-8d9c1d2f3f22', 'templates', 'Templates', 5, '{}', now(), now());
INSERT INTO public.doc_categories (id, key, name, sort_order, visible_roles, created_at, updated_at) VALUES ('ff899115-2656-43f3-ade5-99e3ec8386eb', 'company', 'Company', 1, '{}', now(), now());
INSERT INTO public.doc_categories (id, key, name, sort_order, visible_roles, created_at, updated_at) VALUES ('fc8b87ef-0670-45a7-935e-33007b075cd5', 'pricing', 'Pricing', 3, '{}', now(), now());
INSERT INTO public.doc_categories (id, key, name, sort_order, visible_roles, created_at, updated_at) VALUES ('da6e15a9-276b-415b-88c7-0dd9598ac7b1', 'products', 'Products', 2, '{}', now(), now());
INSERT INTO public.doc_categories (id, key, name, sort_order, visible_roles, created_at, updated_at) VALUES ('e3549c32-a29f-42ca-9784-bd7da5ebd94b', 'technical', 'Technical', 4, '{}', now(), now());

INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('crm.record_change', 0, NULL, 0, 0, 0, NULL, NULL, 'c83f2725-fbaa-429f-85b0-a188e29dbba3', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('lead.promote', 0, NULL, 0, 0, 0, NULL, NULL, '0a6fd352-00dd-45e9-900d-b2437fbd98bd', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('reminder.add', 0, NULL, 0, 0, 0, NULL, NULL, '159b8d23-5ebc-49dd-b0ae-4f7db5636fff', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('quotation.create_draft', 0, NULL, 0, 0, 0, NULL, NULL, 'c61d35f1-1e03-4fa2-be24-b1d4993d0efc', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('reply.draft', 0, NULL, 0, 0, 0, NULL, NULL, '15e3d79e-9682-418b-9de5-15bf977a8985', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('email.reply.send', 0, NULL, 0, 0, 0, NULL, NULL, '330225a3-9597-45c0-bb65-035292173432', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('whatsapp.reply.send', 0, NULL, 0, 0, 0, NULL, NULL, '04fe2a48-9085-4078-b6ea-06d6a0dc780f', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('mcp.crm_write', 0, NULL, 0, 0, 0, NULL, NULL, 'fd28b443-5ec3-4643-8824-9fa085e042ac', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('whatsapp.ack.send', 0, NULL, 0, 0, 0, NULL, NULL, 'eb686db4-47b7-4765-b1ec-de33841e602d', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('email.ack.send', 0, NULL, 0, 0, 0, NULL, NULL, '8e83c1a9-f7b3-4f16-a3de-5866a3dad805', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('reengagement.send', 0, NULL, 0, 0, 0, NULL, NULL, '9385016f-7bc5-413c-9f0e-2baf61750a32', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('contact.merge', 0, NULL, 0, 0, 0, NULL, NULL, 'bec150da-0b52-43aa-9d18-cd7a76268a31', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('identity.link', 0, NULL, 0, 0, 0, NULL, NULL, '1362208a-9d4b-4848-b661-806aa255faef', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('price.answer.send', 0, NULL, 0, 0, 0, NULL, NULL, 'dec2efe6-7762-429d-b703-3e4444d0b54b', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('quotation.send', 0, NULL, 0, 0, 0, NULL, NULL, '892c509c-babd-4e35-9e1b-6af45b57e6ef', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('cadence.followup.send', 0, NULL, 0, 0, 0, NULL, NULL, 'ac10d711-8b55-4e2f-94fe-0237dbc7ff91', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('meeting.slots.send', 0, NULL, 0, 0, 0, NULL, NULL, '05d1f4fe-1627-4929-bb62-6a62890b4013', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('meeting.book', 0, NULL, 0, 0, 0, NULL, NULL, '3ffef288-8779-4b2c-8010-0137185eac8e', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('meeting.reminder.send', 0, NULL, 0, 0, 0, NULL, NULL, '2c8a86c7-49a4-4a97-8a5f-92d27c0714b4', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('summary.email.send', 0, NULL, 0, 0, 0, NULL, NULL, 'c08bb57d-6af5-4e4f-a6c9-2d998eb0944b', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('contract.create_draft', 0, NULL, 0, 0, 0, NULL, NULL, '13986edd-d899-4ccb-b9e5-658ee7869afa', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('proforma.raise', 0, NULL, 0, 0, 0, NULL, NULL, '9e37028f-5e18-451e-8bff-5f2343a342f7', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('handoff.kickoff', 0, NULL, 0, 0, 0, NULL, NULL, 'eb7809b6-edd5-479a-98fd-5e57b5da7733', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('lost.reply.send', 0, NULL, 0, 0, 0, NULL, NULL, '8301865b-c90b-41e0-9b9c-757a034fb8d9', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('partner.route', 0, NULL, 0, 0, 0, NULL, NULL, '47742cae-2d98-412d-bc37-124fa7aeaa90', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('proposal.discovery.start', 0, NULL, 0, 0, 0, NULL, NULL, '73096e97-5f76-4b07-917c-b30131b5b566', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('proposal.create_draft', 0, NULL, 0, 0, 0, NULL, NULL, 'edf0922c-830c-4e40-93fb-b875f1e9d540', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('proposal.send', 0, NULL, 0, 0, 0, NULL, NULL, 'd1afc6b5-9eaf-4504-81cb-6a76739e09d5', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('rates.propose_update', 0, NULL, 0, 0, 0, NULL, NULL, '3a6bd5df-62aa-44a4-bc38-a8d02f209b0f', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('lead.open_returning', 0, NULL, 0, 0, 0, NULL, NULL, '6f7d5844-6dd3-45b4-8ffb-85293644a3d7', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('mcp.proposal', 0, NULL, 0, 0, 0, NULL, NULL, '11bbf592-c554-45d3-8e1c-c0fa4184fe1a', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('lead.merge', 0, NULL, 0, 0, 0, NULL, NULL, '5cc276c9-10cd-473b-bc9f-39a44c78c3c0', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('proposal.from_chat', 0, NULL, 0, 0, 0, NULL, NULL, 'fb24fede-5999-48dc-8efc-7805efd760eb', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('proposal.update_requirements', 0, NULL, 0, 0, 0, NULL, NULL, 'dd761c28-2f16-4a54-abd4-17408352d706', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('organization.merge', 0, NULL, 0, 0, 0, NULL, NULL, '779473d8-9685-49ae-8471-04ef42b613b7', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('contact.delete', 0, NULL, 0, 0, 0, NULL, NULL, 'ceec27d4-5179-486f-85c0-9f515fa855d4', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('message.cancel_scheduled', 0, NULL, 0, 0, 0, NULL, NULL, '28c09cec-7087-4109-9eb5-ab1f9aef181b', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('quotation.update', 0, NULL, 0, 0, 0, NULL, NULL, '1460a581-db84-4c67-bffd-eb64471d5412', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('document.send', 0, NULL, 0, 0, 0, NULL, NULL, 'f162af13-e2d6-4d20-9f2c-176ceceecfba', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('proposal.visual', 0, NULL, 0, 0, 0, NULL, NULL, 'cd91daca-ca2b-47c9-a404-de4ef6ad9f18', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('proposal.edit', 0, NULL, 0, 0, 0, NULL, NULL, 'cd1e1589-6c01-4abc-87cb-38ce158f4ec0', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('proposal.revise', 0, NULL, 0, 0, 0, NULL, NULL, '2add0571-f5a5-49b4-a535-19b2e184c89f', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('proposal.reject', 0, NULL, 0, 0, 0, NULL, NULL, 'ec93aba2-6358-4e77-8d61-b07761801edf', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('proposal.quotation', 0, NULL, 0, 0, 0, NULL, NULL, '2d533eaa-5e83-43c6-a919-8cbf53af9a87', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('proposal.from_sections', 0, NULL, 0, 0, 0, NULL, NULL, '02076683-00ec-47db-aa42-298057bb1681', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('catalog.change', 0, NULL, 0, 0, 0, NULL, NULL, 'f4c2a2b4-2887-404d-9fa9-6215ace36307', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('deal.merge', 0, NULL, 0, 0, 0, NULL, NULL, 'b4860c07-9a66-43fb-b703-75054df3c7e0', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('email.link', 0, NULL, 0, 0, 0, NULL, NULL, 'c3cad6a3-fdcb-43ae-89ec-7339d625fff7', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('activity.void', 0, NULL, 0, 0, 0, NULL, NULL, 'd4e42a41-9eab-4c0c-8c91-a00fed2c5b1d', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('quotation.reprice', 0, NULL, 0, 0, 0, NULL, NULL, 'a215390d-8768-4a90-bad6-96b8ce9d3d8b', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('quotation.withdraw', 0, NULL, 0, 0, 0, NULL, NULL, '58137bc5-2e60-4784-988f-065a311796da', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('quotation.cancel', 0, NULL, 0, 0, 0, NULL, NULL, '81c66203-c67f-4762-b525-4fc393805726', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('quotation.revision', 0, NULL, 0, 0, 0, NULL, NULL, '803bf158-ec61-4606-a432-81bcfeb05bae', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('chat.held_decide', 0, NULL, 0, 0, 0, NULL, NULL, 'aa72bc4e-0e83-49fd-847b-acc759edf8b5', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('lead.from_whatsapp', 0, NULL, 0, 0, 0, NULL, NULL, '1ba668a5-4b9f-4085-beb6-b555ac8525c0', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('deal.won', 0, NULL, 0, 0, 0, NULL, NULL, '615642c0-bea6-45e3-90f7-9a6989dc88e3', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('assessment.update', 0, NULL, 0, 0, 0, NULL, NULL, 'bdae7341-6fa1-487e-ae04-9fbcf6b126b4', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('catalog.product_file', 0, NULL, 0, 0, 0, NULL, NULL, 'dfb4e85a-defe-4935-a1e6-d80bf45f24d3', now(), now());
INSERT INTO public.nary_autonomy (action, streak, candidate_since, approvals_total, edits_total, rejects_total, last_changed_at, last_change, id, created_at, updated_at) VALUES ('quotation.link_deal', 0, NULL, 0, 0, 0, NULL, NULL, 'b3dc531a-4c40-4ed4-9f21-77047e99bf17', now(), now());

INSERT INTO public.nary_role_assignments (sop_role, is_nary, user_id, note, id, created_at, updated_at) VALUES ('BDR', true, NULL, NULL, '1f46e1a6-556a-4b73-99c8-6e4674c2df5b', now(), now());
INSERT INTO public.nary_role_assignments (sop_role, is_nary, user_id, note, id, created_at, updated_at) VALUES ('Admin-Ops', true, NULL, NULL, 'baa3a2d9-8090-48bb-a563-8f8bd14dd630', now(), now());
INSERT INTO public.nary_role_assignments (sop_role, is_nary, user_id, note, id, created_at, updated_at) VALUES ('Finance Admin', true, NULL, NULL, '1fd56788-b1c9-4f9b-8ff9-9592d32f358f', now(), now());
INSERT INTO public.nary_role_assignments (sop_role, is_nary, user_id, note, id, created_at, updated_at) VALUES ('Owner', false, NULL, NULL, '4042f96c-2bd1-4045-83c5-8d1196708236', now(), now());
INSERT INTO public.nary_role_assignments (sop_role, is_nary, user_id, note, id, created_at, updated_at) VALUES ('PM', false, NULL, NULL, 'a917fd11-dd58-4966-96ea-7a8b722f8d3a', now(), now());
INSERT INTO public.nary_role_assignments (sop_role, is_nary, user_id, note, id, created_at, updated_at) VALUES ('Tech Lead', false, NULL, NULL, '735cb250-eb73-4e18-9c5b-cd3f00c2bc4d', now(), now());

INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('marketing:email', 'Email marketing: mailing lists, email campaigns and sending', 'marketing', 'Email marketing: mailing lists, email campaigns and sending', false, '5597f86c-12cc-4e4f-a900-ae88bdd19755', now(), now());
INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('partner_mgmt:approve_commission', 'Approve or cancel partner commissions', 'partner_mgmt', 'Approve or cancel partner commissions', false, '2cbb6be8-4f02-4773-b2ac-ba34bc187c8b', now(), now());
INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('partner_mgmt:pay_commission', 'Mark partner commissions paid', 'partner_mgmt', 'Mark partner commissions paid', false, '2e5311d1-4888-4d0d-bf0e-be342d167196', now(), now());
INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('vendor_mgmt:manage_rfq', 'Run RFQs: record vendor quotes and close', 'vendor_mgmt', 'Run RFQs: record vendor quotes and close', false, '2b5b98f9-c04a-47c4-a4cc-32b6e5ab99e8', now(), now());
INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('sales:price_manual', 'Enter a manual price (custom services, quote-only items, off-list prices)', 'sales', 'Enter a manual price (custom services, quote-only items, off-list prices)', false, 'a657a753-5f9c-4396-8167-50bb41aa8bcf', now(), now());
INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('nary:view', 'See Nary''s runs, approval queue, memory and status', 'nary', 'See Nary''s runs, approval queue, memory and status', false, '8191d68a-5df4-453a-901e-9346d2db0d74', now(), now());
INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('nary:approve', 'Apply, edit or reject the actions Nary proposes', 'nary', 'Apply, edit or reject the actions Nary proposes', false, '224c5077-7710-4647-bab9-e60f674ed834', now(), now());
INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('nary:manage', 'Pause Nary, change autonomy, edit its memory, roles and staff chat links', 'nary', 'Pause Nary, change autonomy, edit its memory, roles and staff chat links', false, '54d16265-f992-4c46-a87c-4029cbb18f99', now(), now());
INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('email:read_staff_mailboxes', 'Have Nary read staff mailboxes', 'email', 'Have Nary search and read any staff member''s personal mailbox (every read is audited)', false, '83fad2b5-6aee-41a1-b0d6-20f144bbc40f', now(), now());
INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('documents:view', 'View documents', 'documents', 'See the document library (each category may be limited to some roles)', false, 'c257f22e-2a9f-45d7-8478-2668b17594e0', now(), now());
INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('documents:manage_all', 'Manage all documents', 'documents', 'Edit any document, reassign owners, approve customer and partner versions, old versions and restore, category access, trace a leaked copy', false, '8717b3b0-ba54-487c-9b28-744a2f702183', now(), now());
INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('documents:admin', 'Administer documents', 'documents', 'Old versions and restore, owner change, category access, trace a leaked copy (no approval)', false, '58c7d80c-049c-4791-8839-ba826d0fffe6', now(), now());
INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('documents:create', 'Create documents', 'documents', 'Create documents and upload new versions of the documents they own', false, '2c842306-a690-4265-9482-d0a120f30163', now(), now());
INSERT INTO public.permissions (code, name, module, description, is_system_permission, id, created_at, updated_at) VALUES ('crm:send_email', 'Send CRM email', 'crm', 'Reply and send from the CRM inboxes (hello@, sales@...) and sync them', false, '349b7612-c996-4204-8afa-d0cb658de5a7', now(), now());

INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('currency.api_key', '', 'string', 'finance', 'ExchangeRate API Key', NULL, false, false, NULL, 'c1c9cd47-ced4-448e-ab44-1bb89aa8476d', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('currency.active', '["USD", "EUR"]', 'json', 'finance', 'Active Currencies', NULL, false, false, NULL, 'f651b283-59d9-4d95-826a-e21eb3209969', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('currency.functional', 'USD', 'string', 'finance', 'Functional Currency', NULL, false, false, NULL, 'da55e3c6-84aa-47e4-940e-1bb7ac836dde', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('hr.face_match_distance', '0.50', 'decimal', 'hr', 'Face match strictness (max distance)', NULL, false, false, NULL, '6632d929-58d6-442d-89ba-0d345c408b1f', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.no_em_dash', 'true', 'boolean', 'ai', 'Remove em dashes from everything the AI writes', NULL, false, false, NULL, 'a5a9b115-3bbe-4a07-a27c-277093a0453e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('hr.geo_max_accuracy_m', '100', 'decimal', 'hr', 'Max GPS inaccuracy for clock-in (m)', NULL, false, false, NULL, '67e6f92d-6c48-4d9e-8138-677b9df283ef', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('payroll.contributions', '[]', 'json', 'hr', 'Payroll contributions', 'Payroll contributions: [{key, label, employee_rate %, employer_rate %, base_cap, employer_taxable, employee_deductible}]', false, false, NULL, 'ba043fe7-e591-4fe1-9789-1626223ed4b6', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('payroll.income_tax', '{"method": "none"}', 'json', 'hr', 'Payroll income tax', 'Payroll income tax: {"method": "none"} or progressive brackets with allowances per tax status', false, false, NULL, '55a4ef5c-d5ce-4f55-9733-d27798fed4f5', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('mail.sync_lock_ttl_seconds', '600', 'int', 'email', 'Mail sync: lock TTL per account folder (seconds)', NULL, false, false, NULL, '6d293181-407c-4b12-ba2b-d4b2cbe59b8a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sign.reminder_window_hours', '48', 'int', 'signing', 'Signing: remind signers when the deadline is within (hours)', NULL, false, false, NULL, '158e5a16-2337-455b-b627-2160bd5bc6f1', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sign.reminder_interval_hours', '24', 'int', 'signing', 'Signing: hours between reminders to the same signer', NULL, false, false, NULL, '47210aae-a591-456a-86f1-f8acb41b05b1', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('catalog.conversion_base_currency', 'USD', 'string', 'sales', 'Catalog: currency other prices are converted from', NULL, false, false, NULL, '941e66ae-1b90-46e2-86cc-3f750167124d', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('catalog.independent_currencies', '["USD"]', 'json', 'sales', 'Catalog: currencies with their own price list (never converted)', NULL, false, false, NULL, '7a540cfb-5bac-41b6-9db7-48d6905a13f2', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.split_payment_min_unit_equivalents', '21', 'decimal', 'sales', 'Payment terms: unit-equivalents from which products are paid in two parts', NULL, false, false, NULL, 'bae5c5c0-dec1-401d-8058-a4f02a829748', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.split_payment_on_order_pct', '60', 'decimal', 'sales', 'Payment terms: share paid on order when split (%)', NULL, false, false, NULL, 'c91b6bec-f198-4c43-b1f1-cd752362017b', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.service_down_payment_pct', '50', 'decimal', 'sales', 'Payment terms: down payment on services (%)', NULL, false, false, NULL, '8a13175b-9880-4838-8f24-ac2d53edaf38', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('fleet.expiry_warning_days', '30', 'int', 'fleet', 'Fleet: days before an insurance/registration expiry or service shows as soon', NULL, false, false, NULL, '2bbc37ec-1ac4-4b17-a461-60b7cc3a04b2', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('finance.gl_autopost_from', '', 'string', 'finance', 'Automatic GL posting from (YYYY-MM-DD; blank = off)', NULL, false, false, NULL, '4e618f9e-261c-4131-ba4a-0a8534038f93', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('procurement.receipt_over_tolerance_pct', '5', 'decimal', 'procurement', 'Goods receipt: over-delivery tolerance (%)', NULL, false, false, NULL, '6a71116a-35d0-4d6b-9e0a-9e8bbdaabefe', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sign.signature_max_bytes', '500000', 'int', 'signing', 'Max signature image size (bytes)', NULL, false, false, NULL, 'e3be26a9-a045-4553-859c-36872f4ff191', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('payroll.overtime_hours_divisor', '173', 'decimal', 'hr', 'Overtime: hourly wage = monthly fixed wage ÷ this', NULL, false, false, NULL, '229c4ce6-5479-475b-831e-71c3fbfc8714', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('payroll.overtime_multipliers', '{"workday": [[1, 1.5], [null, 2]], "rest_day": [[8, 2], [9, 3], [null, 4]]}', 'json', 'hr', 'Overtime multiples: [cumulative hour upper bound, × hourly wage]', NULL, false, false, NULL, 'e636d1e1-970b-429a-a23a-3c93d69e0823', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('payroll.rest_weekdays', '[5, 6]', 'json', 'hr', 'Rest days for overtime (Monday = 0 … Sunday = 6)', NULL, false, false, NULL, '86846f70-c1ff-4046-8f32-49cd3d326647', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('marketing.email_daily_limit', '100', 'int', 'marketing', 'Email campaigns: max emails sent per day (company time)', NULL, false, false, NULL, 'f0292af6-b16a-4ac0-b340-32166b941312', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('marketing.email_min_delay_seconds', '30', 'int', 'marketing', 'Email campaigns: minimum seconds between sends', NULL, false, false, NULL, '554581a3-1a60-44b5-981c-c00b665a5ce9', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('marketing.email_max_delay_seconds', '60', 'int', 'marketing', 'Email campaigns: maximum seconds between sends', NULL, false, false, NULL, 'cd1d38bb-fe10-4f8b-a297-f95503d79f16', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('marketing.queue_lock_ttl_seconds', '180', 'int', 'marketing', 'Email campaigns: sender lease TTL (seconds, must exceed the max delay)', NULL, false, false, NULL, '3b5d87b3-2d58-4391-ae3a-e54dfd8210a3', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('maintenance.due_soon_days', '7', 'int', 'maintenance', 'Maintenance: days before a plan is due that it shows as due soon', NULL, false, false, NULL, '258ee706-1df3-462b-8557-2e7b64424131', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('hr.overnight_shift_hours', '16', 'int', 'hr', 'Attendance: hours a shift may run past midnight and still be clocked out', NULL, false, false, NULL, '04c4b3de-6cfa-4517-a63b-6fb80e22297e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.anthropic.api_key', '', 'string', 'ai', 'Anthropic API key (Claude); blank = ANTHROPIC_API_KEY', NULL, false, false, NULL, 'e89345a7-a41f-4ca1-8c74-f16e015a8a21', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.due_hour', '9', 'string', 'crm', 'Hour of day (company time) rule-made and default follow-ups fall due', NULL, false, false, NULL, '2a13e241-8522-4012-8ecf-96c4be7e32c7', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.escalate_after_hours', '24', 'string', 'crm', 'Email a follow-up still open this many hours after its due notice', NULL, false, false, NULL, 'a51d8046-46a3-4e1a-ac63-f564261efb20', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('projects.monthly_hours', '173', 'decimal', 'projects', 'Working hours in a month (hourly cost = salary ÷ this)', NULL, false, false, NULL, '2a2981e3-fcd7-4913-9043-3869a035ef57', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('projects.default_hourly_cost', '', 'decimal', 'projects', 'Hourly cost for users without an employee record (functional currency)', NULL, false, false, NULL, '91985166-7acc-486b-bcf8-2e6a093894c9', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('finance.invoice_reminder_days', '[1, 7, 14, 30]', 'json', 'finance', 'Overdue invoice reminders: days after the due date', NULL, false, false, NULL, '8345213a-227b-407b-9e39-f23eb18202a9', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('finance.invoice_reminder_repeat_days', '30', 'int', 'finance', 'Then remind every N days (0 = stop)', NULL, false, false, NULL, '760d87fb-b43e-49ef-9ff6-d67f8dc446ae', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('finance.default_sales_tax', '{}', 'json', 'finance', 'Default sales tax by currency (tax code)', NULL, true, false, NULL, '3a08a148-6492-4862-9df7-d8e5a113b610', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('currency.decimals', '{"USD": 2, "EUR": 2, "GBP": 2, "SGD": 2, "JPY": 0, "CNY": 2, "AUD": 2}', 'json', 'finance', 'Decimal places per currency', NULL, true, false, NULL, 'c9097851-c354-45ab-acbd-e268f37a20da', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.scheduled.max_attempts', '12', 'int', 'crm', 'Scheduled messages: attempts before failing', NULL, false, false, NULL, '414c6765-c811-43ec-b055-4b5daeff4b0a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.scheduled.stuck_after_minutes', '30', 'int', 'crm', 'Scheduled messages: fail a send stuck this many minutes', NULL, false, false, NULL, '5a23c662-1cc0-454a-be12-862678c0b048', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.default_delay_days', '3', 'string', 'crm', 'Default follow-up delay (business days)', NULL, false, false, NULL, 'f695d5b5-b4da-4b9c-8748-00d5b987d704', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.no_reply_days', '3', 'string', 'crm', 'Remind when a customer has not replied for (days)', NULL, false, false, NULL, '10d0ad3e-2b53-4a22-a642-8c8bdfe1bbbd', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.lead_idle_days', '7', 'string', 'crm', 'Remind when a lead has had no activity for (days)', NULL, false, false, NULL, '593a217c-bcaa-4374-8c23-cf8436fe3e7c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.deal_stage_idle_days', '14', 'string', 'crm', 'Remind when a deal sits in one stage for (days)', NULL, false, false, NULL, 'ec9a5fc3-c9cf-42fb-ba9b-af73e30c2253', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.quotation_followup_days', '5', 'string', 'crm', 'Remind when a sent quotation has no answer for (days)', NULL, false, false, NULL, 'f062dc0f-fc90-4f15-b1d2-3552fcf665d0', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.max_per_rule_pass', '25', 'string', 'crm', 'Most reminders one rule may raise in a day', NULL, false, false, NULL, '7db08f8c-9ef9-4eaa-a16b-f3090b224f04', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.quiet_from', '08:00', 'string', 'crm', 'Reminders are delivered from', NULL, false, false, NULL, 'e9123c13-38ed-4963-8a84-de7c914e4d43', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.quiet_to', '20:00', 'string', 'crm', 'Reminders are delivered until', NULL, false, false, NULL, '52aa3071-3216-4b4a-a368-e8678bda5cf3', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.digest_weekday', '1', 'string', 'crm', 'Weekly Owner digest weekday (1 = Monday)', NULL, false, false, NULL, '47dd1592-5397-4d28-9fbd-43fc663b547b', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.digest_time', '08:00', 'string', 'crm', 'Weekly Owner digest time', NULL, false, false, NULL, '69ebdc64-68f4-4add-b9cd-34f09ab093e8', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.rule_no_reply', 'true', 'string', 'crm', 'Rule: remind when the customer has not replied', NULL, false, false, NULL, 'd31e48e4-f83f-43ae-9927-37f725999296', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.rule_stale_lead', 'true', 'string', 'crm', 'Rule: remind about idle leads', NULL, false, false, NULL, 'ea3ad2cd-3879-47e7-9ea1-57490c255269', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.rule_stale_deal', 'true', 'string', 'crm', 'Rule: remind about deals stuck in a stage', NULL, false, false, NULL, 'bb554984-3ddb-4b22-b29b-41e4b9d6a544', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.rule_quotation', 'true', 'string', 'crm', 'Rule: remind about sent quotations with no answer', NULL, false, false, NULL, '133d3c52-a1d1-48da-b466-9e4933fde86e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.funnel.stage_probabilities', '{"new": 5, "qualifying": 10, "discovery": 20, "assessment": 35, "proposal": 50, "contract": 80, "won": 100, "handover": 100, "onboarding": 100, "nurture": 5, "lost": 0, "disqualified": 0}', 'json', 'crm', 'Funnel: default probability (%) per deal stage, set when a deal enters the stage', NULL, false, false, NULL, '95016182-815a-4c1a-a3e9-0ae8492d105d', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.ack.default_language', 'en', 'string', 'nary', 'Nary: acknowledgement language when the customer''s can''t be told (id or en)', NULL, false, false, NULL, '4ee166f9-95b2-4557-930f-8a55349f133c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.ack.ongoing_days', '30', 'int', 'nary', 'Nary: a chat with a message in the last N days is ongoing (no acknowledgement)', NULL, false, false, NULL, '88b9907a-8620-48a6-9116-8fccd4ed8604', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.ack.late_after_minutes', '30', 'int', 'nary', 'Nary: an acknowledgement sent later than this after the message starts with the apology', NULL, false, false, NULL, 'e7d6a15c-f995-4638-a923-43645f6e001e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.label_map', '{}', 'json', 'nary', 'WhatsApp: deal stage (or "lead:<status>") → the name of an existing WhatsApp Business label', NULL, false, false, NULL, 'e0dc7283-05e8-45c1-a100-8eea5d87cefc', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.history.full_read_months', '6', 'int', 'nary', 'History import: chats active in the last N months are read in full, older ones summarised', NULL, false, false, NULL, 'fd0b465b-9dec-4437-acc7-3149a819fa13', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.history.chats_per_run', '3', 'int', 'nary', 'History import: chats imported per run (ban risk)', NULL, false, false, NULL, '3af77107-c5c5-4886-b357-2ea3c4dfad05', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.history.page_size', '100', 'int', 'nary', 'History import: messages fetched per request', NULL, false, false, NULL, 'e21c448c-ce68-4b17-97ac-513f9053f8ea', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.history.pause_seconds', '5', 'int', 'nary', 'History import: pause between requests (ban risk)', NULL, false, false, NULL, '148a8023-ffa2-4b1b-a824-cfdbae9562f5', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.history.run_minutes', '10', 'int', 'nary', 'History import: run every N minutes', NULL, false, false, NULL, '20ca520e-e326-4d3c-aa7e-3b658254e128', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.history.pages_per_run', '10', 'int', 'nary', 'History import: message pages fetched per chat per run', NULL, false, false, NULL, '12f372bd-b8d3-4c61-9009-a539b2229d79', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.cadence.min_gap_days', '2', 'int', 'nary', 'Follow-up: never two messages on the same topic within this many days', NULL, false, false, NULL, 'd1464de1-becf-44ef-a37e-c745edc46b0f', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.funnel.loss_reasons', '[{"key": "price_too_expensive", "label": "Price: too expensive", "recontact_months": 6}, {"key": "price_budget_eliminated", "label": "Price: budget eliminated", "recontact_months": 3}, {"key": "competitor_feature", "label": "Competitor: feature", "recontact_months": 6}, {"key": "competitor_relationship", "label": "Competitor: relationship", "recontact_months": 12}, {"key": "competitor_reputation", "label": "Competitor: reputation / trust", "recontact_months": 12}, {"key": "timeline_we_too_slow", "label": "Timeline: our delivery too slow", "recontact_months": 3}, {"key": "timeline_customer_not_ready", "label": "Timeline: customer not ready", "recontact_months": 4}, {"key": "scope_mismatch", "label": "Scope mismatch", "recontact_months": 6}, {"key": "no_decision", "label": "No decision", "recontact_months": 3}, {"key": "lost_contact", "label": "Lost contact", "recontact_months": 12}, {"key": "qualification_error", "label": "Qualification error", "recontact_months": null}, {"key": "procurement_blocker", "label": "Procurement blocker", "recontact_months": 12}]', 'json', 'crm', 'Funnel: loss reasons and re-contact interval in months (null = none)', NULL, false, false, NULL, '0c65e1ca-8e48-4c0c-9017-229b550ff7cd', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.funnel.disqualify_reasons', '[{"key": "free_or_cheap_seeker", "label": "Looking for a free or very cheap alternative", "recontact_months": null}, {"key": "beyond_roadmap", "label": "Needs capabilities beyond our products", "recontact_months": null}, {"key": "below_floor_price", "label": "Expects pricing below our floor price", "recontact_months": null}, {"key": "payment_risk", "label": "Payment default or bad payment history", "recontact_months": null}, {"key": "compliance_geography", "label": "Export control or compliance issue", "recontact_months": null}, {"key": "ip_fishing", "label": "Fishing for IP without purchase intent", "recontact_months": null}, {"key": "timing_only", "label": "Timing only", "recontact_months": 6}, {"key": "not_a_buyer", "label": "Not a buyer (vendor, job seeker, student)", "recontact_months": null}, {"key": "no_response", "label": "Never responded", "recontact_months": null}, {"key": "duplicate", "label": "Duplicate", "recontact_months": null}, {"key": "spam", "label": "Spam", "recontact_months": null}]', 'json', 'crm', 'Funnel: disqualification reasons and re-contact interval in months (null = none)', NULL, false, false, NULL, '1a94e2f1-5b67-4805-bed3-87d128695155', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.funnel.default_lead_owner_email', '', 'string', 'crm', 'Funnel: owner of leads that arrive with nobody behind them (user email)', NULL, false, false, NULL, 'f7cb9a4e-3b82-4bc6-9f63-fae2bfff9629', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.rule_next_action', 'true', 'string', 'crm', 'Rule: remind when an activity next action falls due', NULL, false, false, NULL, '2e7ca550-fac9-45eb-bcdb-468dffad5ef0', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.pricing.batch_discount_pct', '50', 'decimal', 'ai', 'AI prices: Batch API discount (%)', NULL, false, false, NULL, '644ceadd-5c35-44c4-8606-7974ad5e5b86', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.budget.warn_pct', '80', 'decimal', 'ai', 'AI budget: warn the Owner at this share of the monthly cap (%)', NULL, false, false, NULL, '38036565-899c-4181-9797-581b5193b0f3', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.burst_alert_minutes', '5', 'int', 'nary', 'WhatsApp: the window (minutes) of the burst alert above', NULL, false, false, NULL, '1973da09-1d8e-4b13-9934-99a306fd48e0', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.complaint_pause_hours', '72', 'int', 'nary', 'WhatsApp: when a customer says they get repeated messages, Nary''s automatic sends to that chat need your approval for this many hours', NULL, false, false, NULL, '151733ef-f20e-4bca-9e4e-986e984562c0', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.reply.unanswered_business_hours', '2', 'int', 'nary', 'Replies: the daily check lists chats where the customer has had the last word for more than this many working hours', NULL, false, false, NULL, '0eb9439d-6952-4452-9c40-151cbd40b2ac', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.files.page_cap', '200', 'int', 'nary', 'Customer files: the most PDF pages read from one file', NULL, false, false, NULL, '9d46266d-8422-4b3d-b150-0a6ec839d70c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.assistant.system_prompt', '', 'text', 'ai', 'Nary persona for the chat (how Nary thinks and advises)', NULL, false, false, NULL, '8c11188e-f370-4536-a143-663eaf31bade', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.reply.debounce_seconds', '75', 'int', 'nary', 'Replies: wait this many seconds after a customer''s message so a burst of messages gets one reply (0 answers at once)', NULL, false, false, NULL, 'e51fcd03-ffc3-4784-b3af-120fdab279b6', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.budget.stop_pct', '100', 'decimal', 'ai', 'AI budget: stop non-essential jobs at (%)', NULL, false, false, NULL, '7d4ff0e8-6705-421e-bc48-ede1f88d62f3', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.cache.default_ttl', '5m', 'string', 'ai', 'Prompt cache lifetime for jobs without their own (5m or 1h)', NULL, false, false, NULL, '8e2c3f9d-6f58-4e77-8acd-807dd89c5856', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.budget.hard_stop_pct', '150', 'decimal', 'ai', 'AI budget: stop every job, essential ones too, at (%)', NULL, false, false, NULL, '448d4577-11fc-44df-a286-9afcdac56682', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.budget.essential_jobs', '["nary.ack", "nary.reply_draft", "nary.triage", "nary.score", "nary.slot_pick"]', 'json', 'ai', 'AI budget: jobs that keep running past the stop threshold', NULL, false, false, NULL, 'baa2adb8-bf99-489c-8e90-8b950c5c238c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.budget.job_caps', '{}', 'json', 'ai', 'AI budget: optional monthly cap per job (USD)', NULL, false, false, NULL, '11d2d33c-e6a6-4c50-b776-9e00a57d6dc1', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.budget.monthly_usd', '100', 'decimal', 'ai', 'AI budget: monthly cap in USD (0 = no cap)', NULL, false, false, NULL, '8f1ff50c-2344-4d4c-a637-24515e043fc6', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.paused', 'false', 'boolean', 'nary', 'Nary: paused (no runs, no actions; briefs still go out)', NULL, false, false, NULL, '20fa4120-48be-456c-959a-c71287b2de91', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.shadow_mode', 'true', 'boolean', 'nary', 'Nary: shadow mode (customer-facing drafts are stored, never sent)', NULL, false, false, NULL, '34069534-ed79-4581-a901-3f7b5091387c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.autonomy.promotion_count', '10', 'int', 'nary', 'Nary: approvals in a row without a meaningful edit before an action can be promoted', NULL, false, false, NULL, 'cedff4f6-4cbf-4a9b-ace6-a68065be4b2f', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.autonomy.edit_threshold_pct', '10', 'decimal', 'nary', 'Nary: an approver''s edit counts as meaningful above this share of changed text (%)', NULL, false, false, NULL, '320aa89a-9cff-45f4-93ca-8d664e1808b7', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.autonomy.demotion_floor', '1', 'int', 'nary', 'Nary: lowest level an automatic demotion goes to', NULL, false, false, NULL, '8b11ca9d-9db5-4d9e-b03d-749595d6cdc5', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.autonomy.never_promote_unlocked', 'false', 'boolean', 'nary', 'Nary: allow a superadmin to edit the never-promote list', NULL, false, false, NULL, 'bfad3050-39fc-41b3-90d7-0c2757a93106', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.working_hours', '{"days": [1, 2, 3, 4, 5], "start": "08:00", "end": "17:00"}', 'json', 'nary', 'Nary: working hours, company time (days 1 = Monday)', NULL, false, false, NULL, 'c1e18523-aa8d-4118-8931-9e2578afd38e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.heartbeat_minutes', '30', 'int', 'nary', 'Nary: heartbeat every N minutes inside working hours', NULL, false, false, NULL, '134cbeac-b274-4f6d-8069-7d07e03b4754', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.brief_times', '{"morning": "08:00", "midday": null, "wrapup": "17:00"}', 'json', 'nary', 'Nary: brief times, company time (null = off)', NULL, false, false, NULL, 'ac66afc6-abce-473d-9aea-33fa117efd04', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.urgent', '{"hot_score": 10, "customer_wait_minutes": 60, "segments": ["government", "enterprise"], "events": ["quotation.signed", "contract.signed", "payment.recorded"]}', 'json', 'nary', 'Nary: what interrupts the Owner inside working hours', NULL, false, false, NULL, 'fb4d9133-80e9-4e21-84f5-3009be94be4c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.reply.unanswered_lookback_days', '14', 'int', 'nary', 'Replies: the daily check looks at chats whose customer wrote within this many days', NULL, false, false, NULL, '4cdee6f9-07d0-437a-baea-6a1fc3e6855c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.events.batch_size', '50', 'int', 'nary', 'Nary: events handed to subscribers per dispatch', NULL, false, false, NULL, 'fe1562df-9d56-4e2f-a3e9-f1390dc77622', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.events.max_attempts', '5', 'int', 'nary', 'Nary: attempts before an event is parked as failed', NULL, false, false, NULL, 'dac11436-6f3f-4228-9e94-acc3a42082ed', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.pending.expire_days', '7', 'int', 'nary', 'Nary: an approval card nobody decided expires after (days)', NULL, false, false, NULL, '9b660263-c542-475a-a557-6e6215524576', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.errors.dedupe_hours', '24', 'int', 'nary', 'Nary: repeat an error card for the same problem at most every (hours)', NULL, false, false, NULL, '8df7fba9-1d38-4201-8ba5-c961f81e5459', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.shadow_exempt_actions', '["whatsapp.ack.send", "email.ack.send", "meeting.book", "meeting.reminder.send"]', 'json', 'nary', 'Nary: customer actions that still go out in shadow mode (the acknowledgement)', NULL, false, false, NULL, '43dc54d1-c8b7-4b71-b9ce-e7dc7a5f3865', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.ack.enabled', 'false', 'boolean', 'nary', 'Nary: send the automatic acknowledgement (turn on together with turning off WhatsApp''s own greeting)', NULL, false, false, NULL, 'fdb7f7c9-26b6-4b03-b608-75e8995aca17', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.intake.arrival_window_minutes', '120', 'int', 'nary', 'Nary: wait this long for the WhatsApp chat a [whatsapp] web form promised, then tell the Owner', NULL, false, false, NULL, 'e46febd8-c242-47d3-8f9b-0df1934f5eb3', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.intake.currency_by_country', '{"*": "USD"}', 'json', 'nary', 'Intake: quoting currency by the customer''s country (ISO code; * = everyone else)', NULL, false, false, NULL, '041d1af4-ad06-45e5-badd-da60c28824b9', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.web3forms.sender_domains', '["web3forms.com"]', 'json', 'nary', 'Web3forms: sender domains of the form notification emails', NULL, false, false, NULL, '6ba3f4f6-c934-4f75-bc41-84719725a04b', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.web3forms.dkim_domains', '["web3forms.com"]', 'json', 'nary', 'Web3forms: a DKIM signature from one of these domains is required', NULL, false, false, NULL, 'bbf6ae2a-d5cc-4346-ac4d-cb5b1fb153e5', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.web3forms.required_fields', '["name", "submission_source"]', 'json', 'nary', 'Web3forms: fields every submission must have (missing → AI extraction and a warning)', NULL, false, false, NULL, 'c240d7bd-4adc-47a2-8b12-7d53550823e1', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.web3forms.poll_junk', 'true', 'boolean', 'nary', 'Web3forms: also fetch form emails that landed in the Junk folder', NULL, false, false, NULL, '7264158c-f578-4fe3-a499-573847f5f4a8', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.web3forms.routes', '{"inline_whatsapp": "whatsapp", "web3forms": "email", "inline_web3forms": "email", "whatsapp": "whatsapp"}', 'json', 'nary', 'Web3forms: submission_source → journey (email: reply by email; whatsapp: expect the chat)', NULL, false, false, NULL, 'bd4030ca-2e9c-4175-8e07-79265e0271e4', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.identity.default_country', 'US', 'string', 'nary', 'Identity: country for phone numbers written without a country code (ISO code)', NULL, false, false, NULL, '53202d02-5965-46a0-9c47-635f206c94a4', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.identity.prefill_labels', '{"name": ["name"], "company": ["company"], "email": ["email", "e-mail"], "phone": ["phone", "whatsapp"], "product": ["product", "interested in"], "message": ["message"]}', 'json', 'nary', 'Identity: labels in the website''s pre-filled WhatsApp text → field', NULL, false, false, NULL, 'e60110eb-2372-4d47-8eb2-461d3f645cdf', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.identity.thresholds', '{"certain": 90, "likely": 70, "weak": 45}', 'json', 'nary', 'Identity: match score from which a match is certain (auto-link), likely (link + tell), weak (ask)', NULL, false, false, NULL, '13a04da8-9b86-44db-99bf-4b71fc031c5b', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.identity.signal_scores', '{"phone": 100, "text_email": 95, "text_phone": 95, "form_text": 85, "name_company": 75, "email_domain": 50}', 'json', 'nary', 'Identity: score of each matching signal (the best signal counts)', NULL, false, false, NULL, 'b8b3e844-8979-4fb1-a612-c7f194e063c1', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.identity.honorifics', '["mr", "mrs", "ms", "miss", "dr", "prof"]', 'json', 'nary', 'Identity: honorifics ignored when comparing names', NULL, false, false, NULL, '78b76341-7cdf-4816-b2f5-1b9187af1498', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.reply.backfill_window_hours', '24', 'int', 'nary', 'Replies: a message found late (gap check) older than this many hours, or older than our last message in the chat, never gets an automatic reply', NULL, false, false, NULL, 'b5b4dd3e-7953-4030-a29e-deab7e07787a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.chat_max_messages', '2', 'int', 'nary', 'WhatsApp: Nary messages to one chat within the window below before the next one needs your approval', NULL, false, false, NULL, '200a9784-d4da-435b-9817-f7067e32462c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.chat_window_minutes', '10', 'int', 'nary', 'WhatsApp: the window (minutes) of the per-chat limit above', NULL, false, false, NULL, '7161dc2d-804c-4094-aa86-ec698ac70639', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.burst_alert_messages', '3', 'int', 'nary', 'WhatsApp: alert when a chat gets more than this many of our messages within the window below', NULL, false, false, NULL, 'aac6ddb5-0b7b-42d3-a023-95edaa7e9945', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.identity.recent_form_days', '14', 'int', 'nary', 'Identity: a chat can match a [whatsapp] web form sent within this many days', NULL, false, false, NULL, '91627af1-f2c9-4313-a63e-8d79a95e2320', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.triage.example_count', '20', 'int', 'nary', 'Triage: the Owner''s latest corrections shown to the model as examples', NULL, false, false, NULL, 'a932c3b9-bf59-43ce-9c11-271e63dba226', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.triage.business_signals', '["price", "pricing", "quotation", "quote", "demo", "project", "tender", "rfp", "procurement", "invoice", "company", "order", "purchase"]', 'json', 'nary', 'Triage: words that send a spam or personal verdict to the second model for review', NULL, false, false, NULL, '3cf2ffea-b80d-47af-b6ca-c1fc38278fb2', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.signature', '_— Nary_', 'string', 'nary', 'WhatsApp: Nary''s sign-off', NULL, false, false, NULL, '399974e5-1f93-4c1a-89ce-9cdad1bfcf7a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.max_per_hour', '20', 'int', 'nary', 'WhatsApp: most messages Nary sends in an hour (ban risk)', NULL, false, false, NULL, '573464f5-8498-4683-b646-093d67dec530', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.max_per_day', '100', 'int', 'nary', 'WhatsApp: most messages Nary sends in a day (ban risk)', NULL, false, false, NULL, '97c4fb1a-4cd6-4e29-963f-3a81327b804c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.takeover_hours', '24', 'int', 'nary', 'WhatsApp: Nary stays silent in a chat this long after a person replies from the phone', NULL, false, false, NULL, '28da00f3-4241-4218-aa62-a17c8e10416a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.gapcheck_minutes', '30', 'int', 'nary', 'WhatsApp: look for missed messages every N minutes', NULL, false, false, NULL, '62e28812-9ce8-47e4-b528-ff62cdef4823', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.gapcheck_lookback_hours', '48', 'int', 'nary', 'WhatsApp: the missed-message check covers chats active in the last N hours', NULL, false, false, NULL, 'e925ea03-bdc6-4d6e-8b54-cab7e372d8ab', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.health_minutes', '10', 'int', 'nary', 'WhatsApp: check the link and Nary''s queue every N minutes', NULL, false, false, NULL, '5fde70cb-50b1-4546-a2cf-5a0354411688', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.health_backlog_minutes', '15', 'int', 'nary', 'WhatsApp: an event waiting longer than this means Nary is stuck (the Owner is emailed)', NULL, false, false, NULL, 'fcd3adc9-d5aa-485f-b8ac-13b516cce319', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.media_max_mb', '25', 'int', 'nary', 'WhatsApp: largest attachment stored (MB)', NULL, false, false, NULL, 'b67252c4-5faf-4dcb-9d11-232d5f512311', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.card_notices', 'true', 'boolean', 'nary', 'WhatsApp: send approval cards to the Owner''s WhatsApp in working hours (reply 1 apply, 2 view, 3 skip)', NULL, false, false, NULL, '32240089-2842-47af-9cc5-e4c00f121467', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.greeting_hours', '{"morning": 4, "afternoon": 12, "evening": 18}', 'json', 'nary', 'WhatsApp: company-time hour each time greeting starts (Good morning/afternoon/evening), applied when a message is sent', NULL, false, false, NULL, '6bf50977-cb96-452a-9afb-ac7c926f12c4', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.writing.lint_enabled', 'true', 'boolean', 'nary', 'Nary writing check: every customer message is checked before its card; one rewrite, else the problems show on the card and it waits for you', NULL, false, false, NULL, '7f7cee9d-6590-48e6-aeea-529c5127a755', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.briefs.snooze_days', '3', 'integer', 'nary', 'Briefs: "snooze N" on an overdue follow-up pushes it out by this many days', NULL, false, false, NULL, '09437d88-19ef-46b9-8502-8189bc45a003', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.ack.templates', '{"en": {"in_hours": "Hi {name}, thank you for contacting us.\nWe have received your message and will reply within {reply_window}.", "after_hours": "Hi {name}, thank you for contacting us.\nWe have received your message and will reply on the next working day.", "late_prefix": "Apologies for the delayed reply.", "name_fallback": "there", "reply_window": "2 working hours", "email_subject": "Thank you for contacting us"}}', 'json', 'nary', 'Nary: acknowledgement texts per language ({name}, {reply_window}); in hours, after hours, late prefix', NULL, false, false, NULL, 'fde57589-aeb1-49f8-88e2-150261998208', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.labels_enabled', 'false', 'boolean', 'nary', 'WhatsApp: keep WhatsApp Business labels in step with the funnel stage', NULL, false, false, NULL, 'ef09b856-74b2-4b99-9df7-889880fa5d4c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.history.batch_size', '100', 'int', 'nary', 'History import: chats sent to the AI in one batch', NULL, false, false, NULL, 'ce10521e-528e-43c6-924b-83fb11e46678', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.send_at.max_days', '30', 'integer', 'nary', 'Scheduled messages: a message the Owner tells Nary to send later (send_at) may be at most this many days ahead', NULL, false, false, NULL, 'eccedab6-8372-4680-9915-81f396415894', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.history.sync_attempts', '2', 'int', 'nary', 'History import: times older messages are requested from the phone before a chat counts as complete', NULL, false, false, NULL, '3861bd06-c131-4184-9a29-1a23c1e09b99', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.recording_max_mb', '500', 'int', 'nary', 'Meeting recordings: the largest recording file accepted (MB)', NULL, false, false, NULL, '6572e22f-3162-4573-8197-34663b1a50d2', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.recording_poll_seconds', '45', 'int', 'nary', 'Meeting recordings: how often the worker asks speech-to-text how a recording is going (seconds)', NULL, false, false, NULL, 'a905a1a6-89dc-46c6-a230-16e8d68c0a0f', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.notes_max_mb', '40', 'int', 'nary', 'Meeting notes: the largest notes or transcript file accepted (MB)', NULL, false, false, NULL, 'e06d7e06-c018-4169-a4fd-c36673ecb7d5', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.recording_deadline_hours', '12', 'int', 'nary', 'Meeting recordings: a recording not transcribed within this many hours of its upload is given up (the Owner is told)', NULL, false, false, NULL, 'a5f86234-c753-4c02-9597-2e44b1fdbd20', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.recording_max_minutes', '240', 'int', 'nary', 'Meeting recordings: the longest recording Nary transcribes (minutes)', NULL, false, false, NULL, '1a28cd7c-74c9-47fe-9366-42a9b6db82ce', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.recording_resubmits', '2', 'int', 'nary', 'Meeting recordings: times a recording is sent to speech-to-text again when the service lost it (a restart)', NULL, false, false, NULL, '0b56d7e3-d186-4f90-9d84-ad2707efe9a4', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.mailboxes.notify_owner', 'false', 'boolean', 'nary', 'Staff mailboxes: notify the owner when someone else''s Nary read their mailbox (every read is audited either way)', NULL, false, false, NULL, 'd219e96b-0acf-4b12-aa0a-f5c2e67a0057', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.files.vision_max_pages', '30', 'int', 'nary', 'Customer files: the most pages or images of one file AI vision reads (the rest are left out, and said so)', NULL, false, false, NULL, '2880b2a1-e632-450d-ad7e-1b6e09ec3e0e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.files.vision_min_chars', '30', 'int', 'nary', 'Customer files: a PDF page with fewer characters of text than this is read by AI vision (a scan)', NULL, false, false, NULL, '38928f69-4e21-44f3-a276-c84b8a868cf9', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.files.sheet_max_rows', '200', 'int', 'nary', 'Customer files: the most rows kept per spreadsheet sheet', NULL, false, false, NULL, '0b3d42ba-e4e1-42e7-b28d-d52a50b30fa9', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.files.max_mb', '25', 'int', 'nary', 'Customer files: the largest file Nary stores and reads (MB)', NULL, false, false, NULL, 'f38934e6-fa34-4c65-8852-f1924dfd8cf6', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.history.style_sample', '200', 'int', 'nary', 'History import: the Owner''s own business replies read to learn the writing style', NULL, false, false, NULL, 'c16d0208-f07e-4720-9aa1-97ee6e43ffbb', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.history.missed_after_days', '14', 'int', 'nary', 'History import: a business chat left unanswered this long is a missed opportunity', NULL, false, false, NULL, '26d2ebe1-8953-41da-b823-8e61109f61d6', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.stt.max_seconds', '600', 'int', 'nary', 'Speech-to-text: longest voice note transcribed (seconds)', NULL, false, false, NULL, '042343b7-dc57-4138-97d5-e0ad4c63613f', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.sop_revisions.briefs', '{}', 'json', 'nary', 'SOP revisions: what each SOP draft must change', NULL, false, false, NULL, '9ae07105-a616-47bf-be3f-545d8afeb704', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.sop_revisions.codes', '{"related": [], "new": ""}', 'json', 'nary', 'SOP revisions: related SOP codes read as house-format reference, and the code of a new SOP Nary may draft (empty: none)', NULL, false, false, NULL, '4024a235-2abb-440e-be98-ad4e81236bc6', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.score.rubric', '{"budget": {"3": "budget confirmed and sufficient for the scope", "1": "a budget exists, size unknown", "0": "no budget yet, must request it internally", "disqualify": "explicitly looking for free or very cheap alternatives"}, "authority": {"3": "direct decision-maker", "2": "strong influencer with a named decision-maker", "0": "researcher without access to the decision-maker"}, "need": {"3": "a clear, urgent, specific problem our products solve", "1": "general interest, no specific pain", "0": "vague, or outside what we offer"}, "timeline": {"3": "live within 30 days", "2": "30 to 90 days", "1": "90 to 180 days", "0": "no timeline, exploratory"}}', 'json', 'nary', 'Scoring: BANT rubric, points per answer', NULL, false, false, NULL, '60bb4485-ed7f-49e6-acdc-cc0cd2feae2a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.score.classes', '{"hot": [10, 12], "warm": [7, 9], "cool": [4, 6], "cold": [0, 3]}', 'json', 'nary', 'Scoring: class → [lowest, highest] total (0 to 12)', NULL, false, false, NULL, '2b37d849-22ac-42b0-933c-0e8b1b8856b7', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.score.boundary_margin', '1', 'int', 'nary', 'Scoring: a total this close to a class boundary is re-scored by the second model', NULL, false, false, NULL, '6054150f-9564-4ab3-884e-e4d7a1e61f2c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.score.red_flags', '["wants capabilities beyond our products without a custom project", "expects prices far below our floor margin", "a known payment default", "a geography with export-control or compliance issues", "fishing for our methods without purchase intent"]', 'json', 'nary', 'Scoring: fit red flags (consider disqualifying)', NULL, false, false, NULL, '3ca839aa-e4f5-44c9-a645-693a49c27d32', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.score.green_flags', '["has bought similar solutions before", "has a team able to adopt and maintain it", "a use case in an industry we already serve"]', 'json', 'nary', 'Scoring: fit green flags', NULL, false, false, NULL, '859be404-2273-45ac-a41a-735558aac16b', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.files.read_max_chars', '12000', 'int', 'nary', 'Customer files: how much text one read_attachment call returns by default (characters; the rest is paged)', NULL, false, false, NULL, '7150301a-22ec-475d-a32f-fc93361096b8', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.reply.questions', '{"pricing_vague": {"en": ["Which product are you interested in?", "Is it for your company or for a project?"]}}', 'json', 'nary', 'Replies: two qualifying questions per product', NULL, false, false, NULL, 'bb6a9238-b2b2-4b3b-b9cb-7b5224914de0', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.attach.max_files', '5', 'int', 'nary', 'Customer messages: the most files (documents and attachments) one message from Nary may carry', NULL, false, false, NULL, 'dd4eec72-8868-44a2-9142-3303d0cd03dd', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.reply.max_questions', '2', 'int', 'nary', 'Replies: most questions in one message', NULL, false, false, NULL, 'fcb521c3-79f1-4da7-8851-faefb0d6e408', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.reply.max_lines', '5', 'int', 'nary', 'Replies: most lines in one WhatsApp bubble', NULL, false, false, NULL, 'a9b03cf6-9f22-4bf5-bf5f-bcb9eef1e459', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('documents.review_reminder_days', '14', 'int', 'documents', 'Documents: remind the owner this many days before the review date', NULL, false, false, NULL, '8667eeed-9508-4c08-bb27-95aa763cc409', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('documents.max_file_mb', '100', 'int', 'documents', 'Documents: the largest file one version may have (MB)', NULL, false, false, NULL, '9a030bca-0089-4947-806e-81f44ed6bf95', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('documents.watermark.font_size', '7', 'int', 'documents', 'Documents: the footer font size (points)', NULL, false, false, NULL, '76256fe8-cce8-47f0-a77f-74385eeeefd7', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('documents.product_files_migrated', 'false', 'boolean', 'documents', 'Documents: catalog product files were moved into the library (set by the system once)', NULL, false, false, NULL, '13634d70-b682-4de9-9860-e1f09128e149', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.price.catalog_terms', '{"pricing_vague": [""]}', 'json', 'nary', 'Price answers: catalog search words per product (a key of the qualifying questions)', NULL, false, false, NULL, 'dc244767-bec2-456e-818e-e4b3af9bda82', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.email.reply_window_hours', '48', 'int', 'nary', 'Email: only emails received in the last N hours start Nary''s reply playbook (older mail is history)', NULL, false, false, NULL, 'b5136303-b69e-4b76-a294-e8cabfacdc29', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('whatsapp.conversations_cache_minutes', '30', 'int', 'integrations', 'WhatsApp inbox: minutes the last chat list stays available when WhatsApp is slow to answer (0 = never)', NULL, false, false, NULL, '3a6ce522-afcd-4b9b-8a90-0f46ba1c1194', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('proposal.visuals.vision_check', 'true', 'boolean', 'documents', 'Proposal figures: Nary looks at each drawn chart or diagram (overlaps, missing items, wrong flow)', NULL, false, false, NULL, '249e6e43-02ec-44f4-a0e8-eff8ec894695', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('proposal.visuals.custom_svg_max_kb', '300', 'int', 'documents', 'Proposal figures: the largest drawing Nary or your AI client (MCP) makes itself (custom_svg, KB)', NULL, false, false, NULL, '05a52d43-55d3-4bb5-9cad-0eefaab1ba10', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('proposal.visuals.max_attempts', '3', 'int', 'documents', 'Proposal figures: failed checks Nary fixes on its own before the figure is kept with its warnings and the Owner is told', NULL, false, false, NULL, '386d49d3-9629-4cc8-af95-fb7b45645f2d', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('proposal.visuals.default_figures', '["timeline", "architecture"]', 'json', 'documents', 'Proposal figures: the figures every new draft gets (timeline, architecture)', NULL, false, false, NULL, '7c4a393b-344d-4682-b946-ed227fc18dba', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('documents.price_list.document_title', 'Price List', 'string', 'documents', 'Documents: the title of the internal price list document', NULL, false, false, NULL, '41cc206c-c41e-46ec-9222-f3a2804eec6d', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('documents.watermark.customer_text', 'Prepared for {company} · {date} · Confidential', 'string', 'documents', 'Documents: the footer on copies sent to customers and partners ({company}, {name}, {date}, {id})', NULL, false, false, NULL, '49123865-235a-4ab4-864e-d54f15bede23', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('documents.watermark.staff_text', 'Downloaded by {name} · {date} · Confidential · {id}', 'string', 'documents', 'Documents: the footer on copies staff view or download ({name}, {company}, {date}, {id})', NULL, false, false, NULL, '0095b732-f3d2-4ff6-8ef4-5997f3c47574', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.cadence.steps', '[{"day": 1, "channel": "whatsapp", "intent": "confirm the quotation arrived; offer to answer first questions", "topic": "receipt", "news": false}, {"day": 3, "channel": "email", "intent": "one paragraph on our differentiator that matches what they said mattered", "topic": "value", "news": true}, {"day": 7, "channel": "whatsapp", "intent": "ask where they are in the review and whether any question is open", "topic": "process", "news": false}, {"day": 14, "channel": "email", "intent": "a short summary of one comparable deployment (same industry or use case)", "topic": "proof", "news": true}, {"day": 21, "channel": "call", "intent": "call them: validity ends in 9 days; offer to extend, modify or discuss; ask the decision timeline", "topic": "timeline", "news": false}, {"day": 28, "channel": "whatsapp", "intent": "the quotation expires in 2 days; offer to extend the validity", "topic": "expiry", "news": false}, {"day": 30, "channel": "erp", "intent": "the quotation expires: extend, revise or move to nurture", "topic": "expiry_decision", "news": false}]', 'json', 'nary', 'Follow-up: steps after a quotation is sent (day, channel whatsapp/email/call/erp, intent, topic, news)', NULL, false, false, NULL, '90e98b55-be3b-4e24-9e96-791782c99c12', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.cadence.max_without_news', '2', 'int', 'nary', 'Follow-up: most follow-ups in a row without new information', NULL, false, false, NULL, '9d785c60-3736-4c9c-a3f5-5f05d6ec908d', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.cadence.at_risk_silent_days', '7', 'int', 'nary', 'Follow-up: no answer this many days before expiry → the deal is at risk', NULL, false, false, NULL, '15dd9eaf-2e57-4353-b2f3-688bc917e31d', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.cadence.nurture_silent_days', '14', 'int', 'nary', 'Follow-up: silent this long after expiry → propose nurture', NULL, false, false, NULL, '6eff0b5a-85e0-4cfb-8e12-b2663b8ab818', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.cadence.recontact_days', '60', 'int', 'nary', 'Follow-up: re-engagement reminder after moving to nurture (days)', NULL, false, false, NULL, '5cffc43b-b8c1-44fb-8052-f4e527aa4264', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.negotiation.scripts', '{"price_too_high": "Ask the gap between the price and their budget; offer a phased approach or a different scope before any discount.", "comparing_competitors": "Ask their top 2 or 3 decision criteria and position our offer on those; never attack a competitor.", "need_more_time": "Agree a date to revisit and set the reminder.", "internal_approval": "Offer a one-page executive summary for the approver, or a call with them.", "timeline_too_long": "Offer a phased rollout: one site first, scale later.", "on_hold": "Ask when the situation changes and check in then.", "competitor_cheaper": "Ask what exactly they offer at that price; compare support, local presence, warranty and terms like for like."}', 'json', 'nary', 'Negotiation: value-first answers per objection, Owner-only', NULL, false, false, NULL, 'b3c6dfcf-1514-4229-8b89-0591c3860d36', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.negotiation.competitor_checklist', '["same scope and quantity", "what is included and what is extra", "delivery and data location", "warranty length and what it covers", "support hours and local presence", "installation and training included", "licence model and renewal price"]', 'json', 'nary', 'Negotiation: what to compare like for like against a competitor''s offer', NULL, false, false, NULL, '13ed4412-9eec-4249-afc7-b43e48255543', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.margins', '{"products": {"label": "Products", "target": "40", "floor": "10", "families": []}, "services": {"label": "Services", "target": "45", "floor": "10", "families": []}}', 'json', 'sales', 'Margins: target and floor gross margin % per product category, and the catalog families in it (internal)', NULL, false, false, NULL, 'b64fd3b4-4570-494e-b2f0-25e86ca0d12f', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.applying_stale_minutes', '30', 'int', 'nary', 'Nary: a card still applying after this many minutes, with nothing running, is marked failed and you are told', NULL, false, false, NULL, 'ae50c874-986f-49ec-9876-de0ab8f580ac', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.apply_wait_seconds', '45', 'int', 'nary', 'Nary: seconds a request waits for long work (drafting, a card) before answering that it is still working', NULL, false, false, NULL, 'd38d3f72-d43a-4274-9f12-f9d0413ca3b9', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.contract.nudge_days', '3', 'int', 'nary', 'Contract: remind about an unsigned contract every N days', NULL, false, false, NULL, 'c3ad6e52-7b30-4ad6-8500-b6135b4e3fbc', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.contract.won_needs_dp', 'true', 'boolean', 'nary', 'Contract: the deal moves to Won only when the contract is signed and the down payment is paid', NULL, false, false, NULL, '07def6ab-fd3a-44aa-8ac0-a2ead17add4b', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.google.client_id', '', 'string', 'nary', 'Google Calendar: OAuth client ID (Google Cloud console)', NULL, false, false, NULL, 'a0756e68-8fbc-46e7-a631-166b4a284cc9', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.google.client_secret', '', 'string', 'nary', 'Google Calendar: OAuth client secret', NULL, false, true, NULL, '76ac51b7-a183-4a6d-ba2f-46f2c5f067be', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.google.account_email', '', 'string', 'nary', 'Google Calendar: the account meetings are booked in', NULL, false, false, NULL, '2bf6a997-a74c-4ca7-a4e3-5d3817ed82c8', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.google.calendar_id', 'primary', 'string', 'nary', 'Google Calendar: calendar ID', NULL, false, false, NULL, '6b248486-99f2-42d8-b6ab-4d76efd63144', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.rules', '{"duration_minutes": 60, "buffer_minutes": 20, "max_per_day": 3, "start_window": ["09:00", "15:40"], "blocks": [{"days": [1, 2, 3, 4, 5], "from": "12:00", "to": "13:00"}], "workdays": [1, 2, 3, 4, 5], "slots_offered": 3, "horizon_days": 10, "min_notice_hours": 18, "step_minutes": 20, "extended_windows": [["07:00", "09:00"], ["18:00", "21:00"]]}', 'json', 'nary', 'Meetings: length, buffer, per day, start window (company time), blocks, slots offered, horizon, extended windows, reduced-hours start window', NULL, false, false, NULL, '2af2cb73-3173-4f65-8f1d-937280329ecc', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.holidays', '[]', 'json', 'nary', 'Meetings: public holidays (no meetings), as YYYY-MM-DD dates. Update each year', NULL, false, false, NULL, '540996f7-bbfa-456d-89aa-99bc6f9c4863', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.reduced_hours_periods', '[]', 'json', 'nary', 'Meetings: reduced-hours periods (the shorter start window applies)', NULL, false, false, NULL, 'b5fd6968-9926-4d37-821b-03bdd0b13322', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.customer_hours', '{"*": {"days": [1, 2, 3, 4, 5], "start": "09:00", "end": "17:00"}, "SA": {"days": [7, 1, 2, 3, 4], "start": "09:00", "end": "17:00"}, "QA": {"days": [7, 1, 2, 3, 4], "start": "08:00", "end": "16:00"}, "KW": {"days": [7, 1, 2, 3, 4], "start": "08:00", "end": "16:00"}, "OM": {"days": [7, 1, 2, 3, 4], "start": "08:00", "end": "16:00"}, "BH": {"days": [7, 1, 2, 3, 4], "start": "08:00", "end": "16:00"}}', 'json', 'nary', 'Meetings: the customer''s working week by country (ISO; * = default; days 1 = Monday)', NULL, false, false, NULL, '57b2e4e0-70a8-42fe-9c97-22f7ca33e8f4', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.text', '{"en": {"recording": "This meeting may be recorded for our internal notes.", "slots_intro": "Here are some times for an online meeting (60 minutes). Please reply with the number you prefer:", "extended": "outside usual working hours", "reminder_day": "Reminder: our meeting is tomorrow, {when}. Link: {link}", "reminder_hour": "Reminder: our meeting starts in 1 hour ({when}). Link: {link}", "title": "Meeting: {company}"}}', 'json', 'nary', 'Meetings: texts sent to the customer (slots, reminders, recording notice) per language', NULL, false, false, NULL, 'c7011e1e-c3bd-4c64-94dd-ac6412f57f30', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.reminders', '{"day_before_at": "09:00", "hour_before_minutes": 60, "owner_brief_minutes": 30, "no_show_minutes": 15}', 'json', 'nary', 'Meetings: reminder times (day before at HH:MM company time, minutes before) and when a missing customer is a no-show', NULL, false, false, NULL, '2ad89424-223a-41eb-a8a2-49ba326ab19d', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.handoff.kickoff_tasks', '["Internal kickoff: walk the team through the handoff package", "Confirm the site survey or environment access with the customer", "Agree the delivery and installation schedule", "Prepare the onboarding and training plan", "Set up the support channel and introduce the customer to it"]', 'json', 'nary', 'Handoff: kickoff tasks on the new project', NULL, false, false, NULL, 'ea3f7865-88a1-41a7-a983-70aae6f12d49', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.partners.routing', '[]', 'json', 'nary', 'Partners: routing rules [{region, industry, partner_account_id}] (a matching lead is proposed to that partner)', NULL, false, false, NULL, '243528fa-5ca7-4716-9a36-94071a7d2408', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.partners.warn_days', '14', 'int', 'nary', 'Partners: warn this many days before a deal registration''s protection ends', NULL, false, false, NULL, '3e289881-e4bb-4982-8e7b-139b8f3d4e23', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.report.kpi_targets', '{"response_sla_pct": 90, "lead_to_sql_pct": 25, "sql_to_deal_pct": 60, "quote_to_win_pct": 35, "win_rate_pct": 40, "quote_to_decision_days": 30, "loss_reason_pct": 100}', 'json', 'nary', 'Weekly report: SOP KPI targets', NULL, false, false, NULL, 'eccf8bc9-aeb7-45a3-9c9f-c82db7a400b3', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.report.weekday', '1', 'int', 'nary', 'Weekly report: day of the week it comes with the morning brief (1 = Monday)', NULL, false, false, NULL, '364da79d-92e8-49ac-be7c-c61b4c037ba4', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.report.window_days', '90', 'int', 'nary', 'Weekly report: conversion KPIs cover the last N days', NULL, false, false, NULL, '46451a6f-c393-4e6d-9ee6-8f026ed77a24', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.web3forms.interest_map', '{"general": null}', 'json', 'nary', 'Web3forms: interested_in code → product family (a hint; Nary reads the message)', NULL, false, false, NULL, '4b6a56fa-2112-4f81-85c6-0b0c1c5c09d8', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.report.response_sla_minutes', '120', 'int', 'nary', 'Weekly report: a first customer message answered within this many minutes meets the response SLA', NULL, false, false, NULL, '3ac1ba60-9de9-4e09-a40e-1b00cb47d766', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.activity.max_chars', '200000', 'int', 'crm', 'The longest note or activity text the CRM stores (characters); a longer one is refused, never cut', NULL, false, false, NULL, 'ca999f60-444c-4692-8398-93ba32231deb', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('website.lead.alert', 'true', 'boolean', 'integrations', 'Tell the Owner at once (bell and WhatsApp) of each website lead the intake receives (not spam)', NULL, false, false, NULL, '15376cfc-33b0-4382-b463-09872b918d6d', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('documents.proposal.template', '[{"key": "executive_summary", "title": "Executive Summary", "blocks": ["text", "kv"], "guidance": "What the customer needs, what we propose, and a commercial basis table (item, price basis, payment, delivery terms) that only repeats the linked quotation."}, {"key": "requirements", "title": "Requirement Coverage", "blocks": ["coverage"], "guidance": "Every requirement the customer stated, in their words, with a status (included, optional_custom, not_in_scope, to_confirm) and a note. Grouped by theme when there are many."}, {"key": "solution", "title": "Proposed Solution", "blocks": ["text", "table"], "guidance": "What is delivered: a components table (component, quantity, role) and how the parts work together."}, {"key": "client_obligations", "title": "Client Obligations", "blocks": ["text"], "guidance": "What the customer provides for delivery (access, people, data, facilities), each with a bold lead-in. Delivery and acceptance depend on them."}, {"key": "implementation", "title": "Implementation Plan", "blocks": ["table"], "guidance": "Phases with timing and scope, from order to acceptance."}, {"key": "acceptance", "title": "Acceptance", "blocks": ["text"], "guidance": "How the delivery is accepted: the criteria agreed with the customer and when they are checked."}, {"key": "support", "title": "Support and Warranty", "blocks": ["text"], "guidance": "What the quotation includes (support period, warranty) and renewals as quoted. Support hours and language."}, {"key": "commercial", "title": "Commercial Summary", "blocks": ["commercial"], "guidance": "Generated from the linked quotation(s); never typed."}, {"key": "terms", "title": "Terms and Next Steps", "blocks": ["text"], "guidance": "Validity, taxes and exclusions, payment terms as quoted, then numbered next steps."}, {"key": "about", "title": "About Us", "blocks": ["text"], "guidance": "One paragraph from approved company facts, and references only from the knowledge base. A contact line."}, {"key": "approval", "title": "Approval", "blocks": ["stamp"], "guidance": "The approval stamp."}]', 'json', 'documents', 'Proposals: the sections, their headings, first blocks and what Nary writes in each', NULL, false, false, NULL, '84ba25d5-da50-4d6a-8ba5-bb52b2940d4a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('documents.proposal.labels', '{"en": {"kind": "Solution Proposal", "prepared_for": "Prepared for", "attention": "Attention", "prepared_by": "Prepared by", "reference": "Reference", "revision": "Revision", "date": "Date", "validity": "Validity", "validity_value": "{days} days from the date of issue", "classification": "Classification", "requirement": "Requirement", "status": "Status", "notes": "Notes", "figure": "Figure", "included": "Included", "optional_custom": "Optional", "not_in_scope": "Not in scope", "to_confirm": "To confirm", "item": "Item", "amount": "Amount", "payment": "Payment", "total": "Total", "approved": "APPROVED ELECTRONICALLY", "awaiting": "AWAITING APPROVAL", "verify": "Verify with the QR code on this page", "draft": "DRAFT", "header": "Confidential", "page": "Page", "coverage_legend": "Included = delivered in this scope. Optional = outside this scope, available as an add-on."}}', 'json', 'documents', 'Proposals: the document''s fixed words per language (cover, coverage statuses, stamp, header)', NULL, false, false, NULL, '9ed361e1-983d-4026-b3c9-ba73d959ffcd', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('documents.proposal.validity_days', '30', 'int', 'documents', 'Proposals: days a proposal is valid', NULL, false, false, NULL, '985c76cb-5002-450c-879d-9ec2601a3b51', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('documents.proposal.classification', '{"en": "Confidential"}', 'json', 'documents', 'Proposals: classification line per language', NULL, false, false, NULL, '4469fca3-2636-42e1-a06b-33a98158fd7a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('documents.proposal.prepared_by', '{"en": ""}', 'json', 'documents', 'Proposals: the ''prepared by'' line per language', NULL, false, false, NULL, 'cce92777-6cd5-4753-96d0-d5d323c79eaf', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('proposal.discovery.fields', '[{"key": "problem", "label": "Problem and the outcome they want", "ask": "What problem should this solve, and what result would make it a success?"}, {"key": "current_setup", "label": "What they use today", "ask": "What do you use for this today?"}, {"key": "scope", "label": "Scope: sites, users or quantities", "ask": "How many sites, users or units does this cover?"}, {"key": "deployment", "label": "Delivery: on-premise or managed; buy or subscribe", "ask": "Should it run on your premises or as a managed service, and do you prefer to buy or subscribe?"}, {"key": "integration", "label": "Integration with their systems", "ask": "Does it need to connect to another system?"}, {"key": "timeline", "label": "Timeline", "ask": "When should it be live?"}, {"key": "budget", "label": "Budget", "ask": "Is there a budget range for this?"}, {"key": "decision", "label": "Decision process and tender format", "ask": "Who decides, and is it a tender with a required format?"}, {"key": "language", "label": "Document language", "ask": "Which language should the proposal be in?"}]', 'json', 'documents', 'Proposals: the requirement discovery checklist (what Nary asks, one or two at a time)', NULL, false, false, NULL, '9834bdbb-2f69-4c4d-b688-4e5052e61f7e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('proposal.discovery.min_coverage_pct', '60', 'int', 'documents', 'Proposals: the share of the checklist known before Nary drafts (the rest become assumptions)', NULL, false, false, NULL, '9050df90-888d-4b54-9ac1-b16ceb855182', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('proposal.cost.contingency_pct', '15', 'decimal', 'documents', 'Cost breakdown: contingency on labour and non-labour (%)', NULL, false, false, NULL, 'aea6b8ba-fe3c-47bd-ac9d-5e4dc7505ab3', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('proposal.cost.days_per_month', '21', 'int', 'documents', 'Cost breakdown: working days per month (a monthly salary → a man-day)', NULL, false, false, NULL, 'cec1c43b-d0ee-4a9e-a9da-fb21a328845b', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('proposal.cost.load_factor', '1.2', 'decimal', 'documents', 'Cost breakdown: employer load on a gross salary (social security, bonuses) for a man-day cost', NULL, false, false, NULL, '0549dbe3-4c84-4f63-8255-0ed6f9d78449', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('proposal.cost.update_threshold_pct', '10', 'decimal', 'documents', 'Cost breakdown: Nary proposes a rate change only above this difference (%)', NULL, false, false, NULL, '4bca3bd6-0116-46a5-a81b-0412c5f84f5b', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.staff_chat.enabled', 'true', 'boolean', 'nary', 'Nary chat on WhatsApp: linked staff can chat with Nary from their own WhatsApp', NULL, false, false, NULL, 'b196109b-2a56-4dca-843a-74676cae6f92', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.staff_chat.session_hours', '12', 'integer', 'nary', 'Nary chat on WhatsApp: start a new conversation after this many quiet hours', NULL, false, false, NULL, '66a4e172-ee28-4410-ab8c-530a220719eb', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.autosend.min_gap_hours', '12', 'int', 'nary', 'Hours between two automatic messages to one customer (a third is held for the Owner)', NULL, false, false, NULL, '14c62f97-e948-4e3c-a69e-244cc060f8e2', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.deal_dup.window_days', '30', 'int', 'crm', 'Two open deals of one contact or organization opened within this many days are checked as one deal', NULL, false, false, NULL, '6b526c0a-e573-4924-b9b6-021836783fbd', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.video_max_mb', '16', 'int', 'nary', 'WhatsApp: an MP4 video up to this size goes as a playable video, a larger one as a document (MB)', NULL, false, false, NULL, 'd22c9795-bb64-4068-af4d-f89908b31995', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.upload.video_max_mb', '100', 'int', 'nary', 'Videos your AI client (MCP) or Nary upload to a record (upload_file): the largest one (MB)', NULL, false, false, NULL, 'f81bfd3c-dfe7-46a4-a050-188dba48e93f', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.media_timeout_seconds', '240', 'int', 'nary', 'WhatsApp: how long the ERP waits for one file to be sent (seconds)', NULL, false, false, NULL, '759e73ff-04c0-48c5-aa7a-9cb666f93614', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.writing.meeting_offer_patterns', '["\\b(schedule|set up|arrange|book|organi[sz]e)\\b[^.?!\\n]{0,40}\\b(call|meeting|demo|session|meet)\\b", "\\b(hop|jump) on a (quick |short )?call\\b", "\\b(happy|glad|available|free) (to|for) (a |an )?(quick |short |brief )?(call|meeting|demo|chat over video)\\b", "\\b(google meet|video call|zoom call|online meeting|virtual meeting)\\b"]', 'json', 'nary', 'Patterns of a call or meeting offer: Nary''s own follow-ups with one are rewritten (only when the customer asks)', NULL, false, false, NULL, 'c64b934f-941a-4f67-b871-3b8eca0845d2', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.staff_chat.max_bubbles', '3', 'integer', 'nary', 'Nary chat on WhatsApp: at most this many WhatsApp messages per answer (the rest: a link to the ERP)', NULL, false, false, NULL, 'eacd5bd1-aab7-40cc-8d98-a1d391f20a48', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.staff_chat.bubble_chars', '1500', 'integer', 'nary', 'Nary chat on WhatsApp: characters per WhatsApp message', NULL, false, false, NULL, '9e1aa889-8c73-4698-8ab2-d2a62fdaed3c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.email.relink_days', '180', 'int', 'crm', 'Email threads of the last N days are linked to a contact made after the emails arrived', NULL, false, false, NULL, 'adb31da4-6936-475b-b100-9bf9fbaae57a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.staff_chat.voice', 'true', 'boolean', 'nary', 'Nary chat on WhatsApp: transcribe voice notes on this server and answer them', NULL, false, false, NULL, 'e77b020f-ac40-4e86-92f8-a9952beec186', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.owner.enabled', 'true', 'boolean', 'nary', 'Nary and you on WhatsApp: Nary raises approvals, consults, questions, alerts and briefs in your chat', NULL, false, false, NULL, '181e9ee3-8adb-487b-adbc-6ba064780a05', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('storage.link_minutes', '15', 'int', 'storage', 'Files: minutes a download or preview link to a stored file keeps working', NULL, false, false, NULL, '12848395-5d13-4823-a3ca-612d4c86a0be', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('auth.login_ip_max_attempts', '30', 'int', 'system', 'Login: failed attempts allowed per address (all accounts) per window', NULL, false, false, NULL, '095c3c9b-473e-406e-8f8f-47e01f940ef0', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('mcp.pat.max_days_power', '90', 'int', 'mcp', 'MCP access: longest life of a token that can act (days)', NULL, false, false, NULL, 'f81a4a4a-73af-4700-a8f3-821851eed74a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.email.relink_full_hours', '24', 'int', 'crm', 'Hours between two full email relink sweeps (the ones in between look only at what changed)', NULL, false, false, NULL, 'd97f05a6-6323-4125-b27a-72a9132e232a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.owner.urgent_kinds', '["hot_lead", "money", "failure"]', 'json', 'nary', 'Nary and you on WhatsApp: what may reach you outside working hours (hot_lead, money, failure)', NULL, false, false, NULL, '34f099db-47fe-4481-b09d-8aca6c29245a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.owner.bundle_minutes', '5', 'integer', 'nary', 'Nary and you on WhatsApp: things raised within these minutes go out as one message', NULL, false, false, NULL, '1e1abc76-c187-4aa0-9da9-171768a4fed7', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.owner.remind_working_hours', '3', 'integer', 'nary', 'Nary and you on WhatsApp: remind you once after this many working hours without an answer', NULL, false, false, NULL, '9ba1533b-e644-4030-90e6-9fbef8f20cbf', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.owner.default_language', 'en', 'string', 'nary', 'Nary and you on WhatsApp: Nary''s language until you write (id or en); then it mirrors yours', NULL, false, false, NULL, 'f2dd92e9-4ac1-494f-9d56-2405b39b80d8', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.owner.consult_quotations', 'true', 'boolean', 'nary', 'Nary and you on WhatsApp: consult you on a quotation request before drafting it', NULL, false, false, NULL, '59c2607e-e46a-43e5-ad9f-24b4cb6ea46a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.owner.consult_proposals', 'true', 'boolean', 'nary', 'Nary and you on WhatsApp: walk you through a drafted proposal (scope, assumptions, cost, price)', NULL, false, false, NULL, 'ea929b38-958c-4d6a-9736-a0009d5dcb34', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.owner.topics_in_context', '10', 'integer', 'nary', 'Nary and you on WhatsApp: how many open topics Nary keeps in mind when you reply', NULL, false, false, NULL, '30a0dcec-978f-452b-b653-0a40e25e8d19', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.email.intake_enabled', 'true', 'boolean', 'nary', 'Email: Nary reads new mail to the CRM inbox that isn''t from an open lead or deal (triage, lead, reply)', NULL, false, false, NULL, '4f14f061-c08c-4b23-9051-fd6ccab930f4', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.email.skip_rules', '{"list_headers": ["List-Unsubscribe", "List-Id", "List-Post"], "precedence": ["bulk", "list", "junk"], "auto_submitted": true, "sender_prefixes": ["noreply", "no-reply", "no_reply", "donotreply", "do-not-reply", "mailer-daemon", "postmaster", "bounce", "bounces", "newsletter", "news", "marketing", "notification", "notifications", "alerts", "info-noreply"], "sender_domains": []}', 'json', 'nary', 'Email: mail Nary skips before any AI call (list headers, Precedence, Auto-Submitted, sender names and domains)', NULL, false, false, NULL, '787197b1-39f6-4d6f-8e3d-6f71f6f02513', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.email.signature_fields', '{"name": "Nary", "title": "Sales Assistant", "position": "Sales Assistant", "department": "Sales", "email": "", "mobile": "", "phone": "", "city": ""}', 'json', 'nary', 'Email: Nary''s values for the company email signature template (name, title, email, mobile…)', NULL, false, false, NULL, '5c5f6ff6-0336-40fe-9478-ea286c4bfff8', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.funnel.lead_priorities', '[{"key": "low", "label": "Low"}, {"key": "medium", "label": "Medium"}, {"key": "high", "label": "High"}, {"key": "urgent", "label": "Urgent"}]', 'json', 'crm', 'Funnel: lead priorities (JSON list of {key, label})', NULL, false, false, NULL, '8b21abd4-a9df-46cf-b6ac-9c7053ea303e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.activity_types', '[{"key": "call", "label": "Call"}, {"key": "email", "label": "Email"}, {"key": "whatsapp", "label": "WhatsApp"}, {"key": "meeting", "label": "Meeting"}, {"key": "task", "label": "Task"}, {"key": "note", "label": "Note"}, {"key": "demo", "label": "Demo"}]', 'json', 'crm', 'CRM: activity types (JSON list of {key, label})', NULL, false, false, NULL, 'f029cbf4-e1c0-4a46-8feb-b429189eb71a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.staff_chat.max_lines', '8', 'integer', 'nary', 'Nary chat on WhatsApp: most lines in one message; more topics wait for your answer', NULL, false, false, NULL, '1ac91202-1474-4c94-a565-bbba2beee149', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.owner.one_at_a_time', 'true', 'boolean', 'nary', 'Nary and you on WhatsApp: raise one topic at a time; the next comes after you answer (urgent ones still go at once)', NULL, false, false, NULL, '96b40a1e-1a67-445c-835f-7c963cd25eb8', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.briefs.items_per_section', '3', 'integer', 'nary', 'Briefs: items shown per section; reply "more" for the rest', NULL, false, false, NULL, 'd4064f28-68ed-42a0-a2b1-cb6e257236bb', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.assistant.catalog_digest_max_items', '300', 'integer', 'ai', 'AI chat: most products listed in its catalog overview (above this, families only)', NULL, false, false, NULL, '50300a20-b906-4336-96af-3aa552fd72e5', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.erp_guide.enabled', 'true', 'boolean', 'ai', 'AI chat: keep the ERP guide (how the ERP works, built from the code) in AI Knowledge', NULL, false, false, NULL, '95ab1fde-bc58-4801-89b9-5e87cb65371b', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.dup.repropose_days', '30', 'int', 'crm', 'Days after which a duplicate deal or lead pair whose merge card expired, failed or was withdrawn is proposed again', NULL, false, false, NULL, 'e52c1ab3-28cc-440e-a2e1-d6fe73147a31', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.email.relink_interval_minutes', '15', 'int', 'crm', 'Minutes between two sweeps that link email threads to contacts made after the emails arrived', NULL, false, false, NULL, 'c0f36b2c-0368-4b4c-816f-9e11022b516e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.events.resume_max_age_hours', '24', 'int', 'nary', 'Nary: on resume, events that arrived while paused and are older than this (hours) are skipped', NULL, false, false, NULL, '2885df9f-ba79-4924-8297-d5a5b2087fbd', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.upload.max_mb', '10', 'int', 'nary', 'Files your AI client (MCP) or Nary upload to a record (upload_file, a saved preview): the largest one (MB)', NULL, false, false, NULL, '5d3b2f6a-76a4-4933-a578-d7adbfdf339f', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.skills.max_in_prompt', '30', 'integer', 'nary', 'Nary skills: most approved skills listed in the chat''s prompt (the rest are found with use_skill)', NULL, false, false, NULL, 'cb3fe630-51c4-4ad9-8a66-07ebc8388517', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.skills.tool_ideas_project', '', 'string', 'nary', 'Nary skills: the R&D project (name) where Nary files ideas for tools it lacks', NULL, false, false, NULL, '52ca7970-bffb-48ba-a108-f4e7bbeeaa55', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.email.link_days', '7', 'int', 'nary', 'Email: a file over the email limit goes as a download link valid this many days (at most 7)', NULL, false, false, NULL, '4723c7ba-dacd-467b-9035-ed5d06b0556e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.attach.max_total_mb', '20', 'int', 'nary', 'Customer messages: the largest total size of the files on one message (MB); on email a larger stored file goes as a download link, WhatsApp videos have their own limit', NULL, false, false, NULL, '00f13d7b-5658-4e80-a7a6-5d20ed0a43b4', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('proposal.price_text_sections', '["commercial", "terms"]', 'json', 'documents', 'Proposal authoring: sections whose text may state money (Owner or sales:apply_discount only, logged)', NULL, false, false, NULL, '887f181d-2e0a-4b7e-af40-eec8df358c43', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('proposal.required_figures_by_requirement', '{"network": "network", "multi_site": "network", "process_flow": "process", "integration_flow": "process"}', 'json', 'documents', 'Proposal figures: requirement sheet flags and the figure each makes required before approval', NULL, false, false, NULL, 'a77fb608-24db-4e81-a006-b9f4612029c3', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.style.refresh_days', '30', 'integer', 'nary', 'Nary style guide: re-learn your WhatsApp and email style every N days (proposed for your approval)', NULL, false, false, NULL, 'c12ac62c-2ea8-4cf0-b99c-39325035aa6c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('mcp.oauth.access_minutes', '60', 'integer', 'ai', 'MCP access: minutes an agent''s login token lasts (it renews itself)', NULL, false, false, NULL, 'a225cca5-4e7d-461a-822a-afbd7c2a8f10', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('mcp.oauth.refresh_days', '30', 'integer', 'ai', 'MCP access: days an agent stays logged in without being used', NULL, false, false, NULL, '383f6f72-6e20-4112-81bb-7052f4033fa6', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.notes.warranty_years', '1', 'int', 'sales', 'Quotation notes: hardware warranty from delivery, years', NULL, false, false, NULL, 'f0b4aa30-bf7f-46b9-b8e1-fc3e1982397c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('mcp.oauth.code_minutes', '10', 'integer', 'ai', 'MCP access: minutes a login code is valid during sign-in', NULL, false, false, NULL, '53e756ea-6da2-4fff-b4dc-cbec0154e838', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('mcp.pat.default_days', '90', 'integer', 'ai', 'MCP access: default lifetime of a personal token (days)', NULL, false, false, NULL, '580a289f-7112-4854-8647-26621f0d5dbe', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('mcp.owner.confirm_minutes', '10', 'integer', 'ai', 'MCP full power: minutes a confirm code stays valid (the Owner''s yes to a message, meeting or invoice)', NULL, false, false, NULL, '64bae382-b614-4f5b-9962-b5013a2bfbe9', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('mcp.apply.require_totp', 'true', 'boolean', 'ai', 'MCP full power: nary_apply asks for the code from the authenticator app (customer messages, invoices, meetings, money, deletes and merges)', NULL, false, false, NULL, '3cf4b0e4-ef03-4209-9257-f7a1cc2682d3', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.staff_chat.wait_seconds', '120', 'integer', 'nary', 'Nary chat on WhatsApp: a message waits up to this many seconds for Nary''s answer to the one before it', NULL, false, false, NULL, 'c8e826cf-497c-47d1-bfe7-8066aa357c3b', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.staff_chat.late_reply_hours', '24', 'integer', 'nary', 'Nary chat on WhatsApp: a message the live feed missed is still answered (marked late) within this many hours', NULL, false, false, NULL, 'd7b49672-ab2d-4521-a605-4e0659f0fc5f', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.wa.gapcheck_grace_seconds', '120', 'integer', 'nary', 'WhatsApp: the missed-message check leaves messages younger than this to the live feed', NULL, false, false, NULL, '609ad884-ee7f-45e8-8052-e019cccb0d15', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.owner.quiet_minutes', '3', 'integer', 'nary', 'Nary and you on WhatsApp: a new topic waits until you have been quiet this many minutes (urgent ones don''t)', NULL, false, false, NULL, 'd5d5b3ee-0561-47e6-ad0d-0f4afd398780', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.owner.alert_stale_hours', '6', 'integer', 'nary', 'Nary and you on WhatsApp: an alert still waiting to be sent after this many hours is dropped as old news', NULL, false, false, NULL, '23ad7560-d3e8-4ff0-ab69-61dbd6a45ee6', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.followup.digest_max_items', '10', 'string', 'crm', 'Follow-ups that fall due together arrive as one WhatsApp listing this many', NULL, false, false, NULL, '3909dc37-3e3e-4e87-9105-e1f50b333404', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.expire_grace_days', '0', 'integer', 'sales', 'Days after its validity ends before a sent quotation is marked expired (0 = the day after)', NULL, false, false, NULL, 'ab7f4622-96fa-40b5-a03f-69faacd86efe', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.expired_nudge_days', '14', 'integer', 'sales', 'Re-quote reminder only for quotations whose validity ended within this many days (older ones expire quietly)', NULL, false, false, NULL, '469093ac-12a2-4daa-9d44-438a257c298a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.reply_alerts.enabled', 'true', 'boolean', 'nary', 'Reply alerts: tell the Owner when a customer answers one of our messages', NULL, false, false, NULL, '8fe98135-41b7-4705-8831-65ab627784b1', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.reply_alerts.mode', 'action_needed', 'string', 'nary', 'Reply alerts: action_needed (only replies that need something from us; the rest go in the brief) or every_reply', NULL, false, false, NULL, '8b3ffbf8-f8e5-43a1-85f0-98cdf7a57481', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('auth.totp.max_failed_attempts', '5', 'int', 'system', 'Two-factor sign-in: wrong codes allowed per user per window', NULL, false, false, NULL, 'a3d789a6-2356-46e3-99a0-c259f624960e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('auth.totp.failed_window_seconds', '900', 'int', 'system', 'Two-factor sign-in: wrong-code window (seconds)', NULL, false, false, NULL, '8c54498a-e2a2-4b59-ad33-5ceca31bd8f0', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('auth.totp.recovery_codes', '10', 'int', 'system', 'Two-factor sign-in: recovery codes made at setup', NULL, false, false, NULL, '237e9c7f-e36c-4ff3-a724-05c518380f1a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('auth.totp.challenge_seconds', '300', 'int', 'system', 'Two-factor sign-in: seconds to enter the code after the password', NULL, false, false, NULL, '55a9bd03-fe2d-419f-92b4-0ae3c9ef5619', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('manufacturing.label.width_mm', '62', 'int', 'manufacturing', 'Unit label width (mm)', NULL, false, false, NULL, '36684f7e-8724-47f6-a7fd-de6832028db8', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('auth.totp.issuer', 'Granary', 'string', 'system', 'Two-factor sign-in: the name authenticator apps show', NULL, false, false, NULL, '0aee9122-1aa0-45e7-bc0e-2d81885483ae', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('manufacturing.label.height_mm', '29', 'int', 'manufacturing', 'Unit label height (mm)', NULL, false, false, NULL, '322773ee-6aa5-4ad4-a117-eb0d05be082c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('manufacturing.serial.generate_tries', '5', 'int', 'manufacturing', 'Unit serials: draws before giving up when a generated serial is taken', NULL, false, false, NULL, '1836dc57-6493-410c-bc6a-1bffae28a1f3', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('documents.seal.outbound', 'true', 'boolean', 'documents', 'Seal every approved outbound PDF (quotations, invoices, orders, proposals) with the company''s digital seal', NULL, false, false, NULL, '9b14d6b4-aecb-4977-b1b5-3134d4b56f70', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.queue.nudge_items', '5', 'int', 'nary', 'Work queue: items listed in a nudge', NULL, false, false, NULL, '353d8a07-677e-4e4a-8c03-1ba591853b0b', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.queue.urgent_gap_minutes', '30', 'int', 'nary', 'Work queue: minutes at least between two nudges for urgent items', NULL, false, false, NULL, 'efb27e00-57ce-4d27-b046-e91b62ce340e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.queue.card_expiry_hours', '24', 'int', 'nary', 'Work queue: pending cards listed as expiring this many hours before they expire', NULL, false, false, NULL, '92b57b60-5243-4a88-a7c1-2698dc7ccade', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.queue.quotation_expiry_days', '7', 'int', 'nary', 'Work queue: sent quotations listed this many days before they expire', NULL, false, false, NULL, 'bfeb18be-9e48-44e6-96bd-86b19c81c3f7', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.queue.meeting_brief_hours', '3', 'int', 'nary', 'Work queue: booked meetings starting within this many hours with no brief for the Owner', NULL, false, false, NULL, '5349074d-bb24-4a14-9d15-77331baca623', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.queue.meeting_reminder_hours', '36', 'int', 'nary', 'Work queue: booked meetings starting within this many hours with no reminder sent', NULL, false, false, NULL, 'a8a35f03-85a2-4c97-9dfb-1fb5e4270cd4', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.staff_chat.card_window_minutes', '60', 'integer', 'nary', 'Nary chat on WhatsApp: minutes a card list stays answerable by number after Nary wrote something else', NULL, false, false, NULL, 'c5fba000-4d26-4f37-9db9-e6b1a577e914', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.queue.nudge_enabled', 'true', 'boolean', 'nary', 'Work queue: nudge the Owner on WhatsApp', NULL, false, false, NULL, 'e86d01e7-e799-495d-b5ec-551715c5ad22', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.queue.new_lead_days', '14', 'int', 'nary', 'Work queue: new or triaging leads listed for this many days', NULL, false, false, NULL, 'f85ffc4d-89cf-4e01-a1e1-70187cd03f80', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.queue.default_limit', '50', 'int', 'nary', 'Work queue: items listed by default', NULL, false, false, NULL, '41929383-58ec-4ef9-893c-6be088f03980', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.queue.nudge_times', '["08:00", "13:00", "17:00"]', 'json', 'nary', 'Work queue: nudge times on working days (company time, HH:MM)', NULL, false, false, NULL, '0c3cfd8c-de55-4814-948e-efafeff71bc5', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.queue.urgent_kinds', '["whatsapp_reply", "email_reply", "new_lead", "unknown_chat"]', 'json', 'nary', 'Work queue: kinds that are urgent (nudged at once)', NULL, false, false, NULL, '994e34e7-1c05-420a-bfed-180af3114751', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.enrich.on_create', 'true', 'string', 'crm', 'Research a new company as soon as it is created', NULL, false, false, NULL, '1368f385-e4a7-4e06-a73a-50f11c53b49d', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.enrich.fields', 'website,industry,country,city,employee_count,annual_revenue,description', 'string', 'crm', 'Fields enrichment may fill (empty ones only)', NULL, false, false, NULL, '2af0bb7c-eae3-42bf-95ac-8172af73d3be', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.enrich.min_confidence', '70', 'string', 'crm', 'Confidence a value needs to be written (0-100)', NULL, false, false, NULL, 'fe1730fb-ee0d-4200-9e14-ffd7fb495a02', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.enrich.refresh_days', '30', 'string', 'crm', 'Re-research a company after (days)', NULL, false, false, NULL, '9ae6137b-8e52-46c0-9eed-57e187f130ff', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.enrich.daily_limit', '50', 'string', 'crm', 'Most companies refreshed in one day', NULL, false, false, NULL, '628c5e4a-d332-4041-b439-6adb1b2d1925', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.find_max_days', '30', 'int', 'nary', 'Finding free meeting slots by hand (find_meeting_slots): the furthest ahead it looks (days)', NULL, false, false, NULL, '4bffe396-fe19-4f05-bbf4-9454555042b4', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.meetings.find_max_slots', '8', 'int', 'nary', 'Finding free meeting slots by hand (find_meeting_slots): the most slots one answer lists', NULL, false, false, NULL, 'ee2bfb8f-15ac-41d7-a621-2bf784a367ce', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.queue.today_kinds', '["followup_due", "card_expiring", "meeting_reminder", "meeting_brief", "accepted_no_contract", "ready_to_win", "approval_overdue", "quotation_expiring", "assessment_new"]', 'json', 'nary', 'Work queue: kinds that are for today', NULL, false, false, NULL, 'f5fa7fa1-fedd-4a8d-8c96-ed4b9a60b96a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.notes.non_payment_pause_days', '30', 'int', 'sales', 'Quotation notes: days an invoice is overdue before work pauses (projects, pilots)', NULL, false, false, NULL, '56b1a63e-c553-4a98-8fd2-93bd039d2049', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.notes.block_submit', 'true', 'boolean', 'sales', 'Quotation notes: a draft whose notes fail the check cannot be submitted for approval', NULL, false, false, NULL, 'fdcee90a-f5c1-458d-afd3-6478b25ffde2', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.notes.non_payment_terminate_days', '60', 'int', 'sales', 'Quotation notes: days an invoice is overdue before the company may terminate (projects, pilots)', NULL, false, false, NULL, '24b9729f-938c-4a24-b321-af224adf078c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.notes.invoice_due_days', '14', 'int', 'sales', 'Quotation notes: invoices payable within this many days', NULL, false, false, NULL, '0f4c0a6a-bd9a-477b-8199-58b63d03880e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.pricing_rules.block_submit', 'true', 'boolean', 'sales', 'Pricing rules: a draft that breaks a blocking pricing rule cannot be submitted for approval without the Owner''s override', NULL, false, false, NULL, '7a8d863d-6038-434d-a09d-8a0312c6c689', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.pricing_rules.min_cost_multiple', '1.15', 'decimal', 'sales', 'Pricing rules: a line sold below this multiple of its full cost (cost price + labour + licence value) is refused at approval (default 1.15, a 15% markup)', NULL, false, false, NULL, 'd320d942-7a65-4430-89f5-50177b184839', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.pricing_rules.owner_approval_skus', '["ENG-RETAINER-*"]', 'json', 'sales', 'Pricing rules: SKU patterns that send every quotation with them to the Owner for approval', NULL, false, false, NULL, '4d6967c1-ea58-4fb3-8f6d-84031e566aea', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('procurement.default_withholding_rate', '0', 'decimal', 'procurement', 'Withholding tax rate for new vendors that withhold it (%)', NULL, false, false, NULL, '6c373ebd-1d0a-4e2e-ad48-2a0106bec90b', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('rnd.gitlab.pull_minutes', '15', 'int', 'integrations', 'GitLab issue sync: minutes between pulls of recently updated issues (catches a missed webhook)', NULL, false, false, NULL, '9a539df2-4618-4b4e-b3d2-9b71d59dc635', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('rnd.gitlab.retry_max_minutes', '60', 'int', 'integrations', 'GitLab issue sync: longest wait before an item that failed is tried again', NULL, false, false, NULL, '4e1b7fa8-71cb-4432-8c9d-897c6954ceb8', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('rnd.gitlab.sync_batch', '50', 'int', 'integrations', 'GitLab issue sync: items and comments sent per repository each minute', NULL, false, false, NULL, '3e604494-1248-42b2-9d4b-1a8909efa4c1', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('files.upload_link.requests_per_minute', '60', 'int', 'nary', 'Upload links: requests per minute allowed per address and per link on the drop page', NULL, false, false, NULL, '25870cd5-437b-409b-84c5-c0da694af3e7', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('files.upload_link.minutes', '30', 'int', 'nary', 'Upload links (files from your AI client (MCP)): how long one link works (minutes)', NULL, false, false, NULL, 'b3c13ff5-f2f9-44c4-a11e-66e38798c73e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('files.upload_link.max_files', '10', 'int', 'nary', 'Upload links (files from your AI client (MCP)): the most files one link takes', NULL, false, false, NULL, '2dc4b7ba-830d-41d3-992e-8b64c085fdf7', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('hr.face.step_timeouts_ms', '{"find": 8000, "action": 10000, "return": 5000}', 'json', 'hr', 'Face check: time allowed per step in milliseconds (find and hold still, do the action, face the camera again)', NULL, false, false, NULL, '03eca502-5438-4b36-b251-1298dcfb38ad', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('hr.face.challenge_ttl_seconds', '60', 'int', 'hr', 'Face check: how long a phone''s liveness challenge stays valid (seconds)', NULL, false, false, NULL, '53e90e65-483e-4de6-bc03-52810eb63fd4', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('hr.face.passive.flash_count', '5', 'number', 'hr', 'Face check: how many colours the screen flashes (4 to 6)', NULL, false, false, NULL, '01afdc3e-9ec3-4c31-922b-afe17b6e2503', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('hr.face.passive.flash_min_corr', '0.6', 'decimal', 'hr', 'Face check: how closely the face colour must follow the flashed colours (correlation 0 to 1)', NULL, false, false, NULL, 'e51be69c-c0d0-489b-95d2-eda9bf920164', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('hr.face.passive.enabled', 'true', 'boolean', 'hr', 'Face check: try the passive check (screen colour flash) first, the blink or head turn only when it is not sure', NULL, false, false, NULL, 'c3a48253-9779-432f-9cbf-58540a88b919', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('hr.face.passive.spoof_min_real', '0.7', 'decimal', 'hr', 'Face check: smallest real-face score of the anti-spoof model (0 to 1)', NULL, false, false, NULL, '32a8fffd-2e45-4eeb-bad9-61d0ee337ebe', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('hr.face.passive.palette', '[[255,120,120],[120,255,140],[120,150,255],[255,230,120],[220,130,255],[120,240,240]]', 'json', 'hr', 'Face check: the soft screen colours the flash picks from, [r, g, b] each 0 to 255 (at least 4)', NULL, false, false, NULL, 'b6322a75-1176-4b60-a808-a209fd5965b6', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('hr.face.passive.flash_ms', '400', 'number', 'hr', 'Face check: how long each colour stays on, in milliseconds (at least 250: under 3 flashes per second)', NULL, false, false, NULL, 'bb75e4ac-4f7b-4146-adfd-8d24e1fd765f', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('hr.face.passive.settle_ms', '340', 'number', 'hr', 'Face check: when the photo is taken after each colour comes on, in milliseconds (shorter than the flash; late is better, phone cameras lag)', NULL, false, false, NULL, '2a7b0b51-3a98-47d5-bf5b-1191efbd517e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('hr.face.passive.flash_min_delta', '1.5', 'decimal', 'hr', 'Face check: smallest colour response on the face (chromaticity units, 0 to 255); below it the check is not sure', NULL, false, false, NULL, 'b82bd073-aec9-4d6d-8034-46dc523fab81', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('proposal.cost.min_monthly', '0', 'decimal', 'documents', 'Cost breakdown: the minimum monthly wage (update each year), the floor of any rate', NULL, false, false, NULL, 'eb06299f-d85a-426d-aa81-aed145a945cc', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.notes.lead_days_min', '10', 'int', 'sales', 'Delivery from (working days)', NULL, false, false, NULL, '1bdd3b70-a076-4289-a073-4d7ca07c4b45', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.notes.lead_days_max', '15', 'int', 'sales', 'Delivery until (working days)', NULL, false, false, NULL, '3f72fcdb-163f-4318-bffb-5c44897bb0cf', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.notes_template.products', '{"name": "Products (goods and licences)", "detect": {"product_types": ["physical", "software", "bundle"]}, "clauses": [{"key": "validity", "label": "Validity", "kind": "fixed", "required": true, "when": "always", "wording_en": "This quotation is valid for {validity_days} days from the issue date.", "guidance": "validity_days is the quotation''s own validity (Settings: quotation_validity_days)."}, {"key": "currency", "label": "Currency", "kind": "fixed", "required": true, "when": "always", "wording_en": "All prices are stated in {currency_name} ({currency}) and are net prices.", "guidance": "Filled from the quotation''s currency."}, {"key": "incoterm", "label": "Incoterm and Freight", "kind": "fixed", "required": true, "when": "foreign+goods", "wording_en": "Prices are quoted ex works. Freight and cargo insurance to the delivery site are quoted separately once the order is confirmed.", "guidance": "Quotations in a foreign currency that ship goods. An indicative freight range may be added (notes_deal).", "extend": true}, {"key": "incoterm_remote", "label": "Incoterm and Freight", "kind": "fixed", "required": true, "when": "foreign+no_goods", "wording_en": "Software and services are delivered remotely; no shipment.", "guidance": "Quotations in a foreign currency with nothing to ship."}, {"key": "scope", "label": "Scope", "kind": "deal", "required": true, "when": "always", "wording_en": "{quantity} units of {product} for {purpose}, {inclusions} at {site}.", "guidance": "One or two sentences: what is supplied, how many, where, and what is included. State the assumptions the price rests on. Never name a person; the customer may be named."}, {"key": "price_basis", "label": "Price Basis", "kind": "deal", "required": false, "when": "always", "wording_en": "Prices follow our current price list.", "guidance": "Only when the price differs from a figure the customer saw earlier, or when list prices are discounted. A discount only when the Owner or someone with sales:apply_discount asks for it."}, {"key": "delivery", "label": "Delivery", "kind": "fixed", "required": true, "when": "goods", "wording_en": "Delivery within {lead_days} working days after the order and its payment are confirmed.", "guidance": "Lead time from Settings (sales.quotation.notes.lead_days_min / _max).", "extend": true}, {"key": "payment_terms", "label": "Payment Terms", "kind": "fixed", "required": true, "when": "always", "wording_en": "{payment_terms_text}", "guidance": "Built from the lines: products, services (with milestones), subscriptions, travel; then the invoice due days.", "extend": true}, {"key": "tax", "label": "Tax", "kind": "fixed", "required": true, "when": "always", "wording_en": "{tax_sentence}", "guidance": "From the lines'' taxes and the quotation''s currency."}, {"key": "warranty", "label": "Warranty", "kind": "fixed", "required": true, "when": "goods", "wording_en": "Goods carry a {warranty_period} warranty against manufacturing defects from delivery.", "guidance": "Warranty years from Settings (sales.quotation.notes.warranty_years).", "extend": true}, {"key": "order", "label": "How to Order", "kind": "fixed", "required": true, "when": "always", "wording_en": "To order, please send a signed copy of this quotation or a purchase order by email.", "guidance": "Always the last item."}]}', 'json', 'sales', 'Quotation notes template: Products (goods and licences)', NULL, false, false, NULL, '1f35e835-d5c1-4221-b87e-b1cacad182f1', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.notes_template.services', '{"name": "Services and projects", "detect": {"product_types": ["service"], "keywords": ["installation", "implementation", "integration", "development", "training", "consulting"]}, "clauses": [{"key": "validity", "label": "Validity", "kind": "fixed", "required": true, "when": "always", "wording_en": "This quotation is valid for {validity_days} days from the issue date.", "guidance": "validity_days is the quotation''s own validity (Settings: quotation_validity_days)."}, {"key": "currency", "label": "Currency", "kind": "fixed", "required": true, "when": "always", "wording_en": "All prices are stated in {currency_name} ({currency}) and are net prices.", "guidance": "Filled from the quotation''s currency."}, {"key": "incoterm", "label": "Incoterm and Freight", "kind": "fixed", "required": true, "when": "foreign+goods", "wording_en": "Prices are quoted ex works. Freight and cargo insurance to the delivery site are quoted separately once the order is confirmed.", "guidance": "Quotations in a foreign currency that ship goods. An indicative freight range may be added (notes_deal).", "extend": true}, {"key": "incoterm_remote", "label": "Incoterm and Freight", "kind": "fixed", "required": true, "when": "foreign+no_goods", "wording_en": "Software and services are delivered remotely; no shipment.", "guidance": "Quotations in a foreign currency with nothing to ship."}, {"key": "scope", "label": "Scope", "kind": "deal", "required": true, "when": "always", "wording_en": "{quantity} units of {product} for {purpose}, {inclusions} at {site}.", "guidance": "One or two sentences: what is supplied, how many, where, and what is included. State the assumptions the price rests on. Never name a person; the customer may be named."}, {"key": "price_basis", "label": "Price Basis", "kind": "deal", "required": false, "when": "always", "wording_en": "Prices follow our current price list.", "guidance": "Only when the price differs from a figure the customer saw earlier, or when list prices are discounted. A discount only when the Owner or someone with sales:apply_discount asks for it."}, {"key": "client_obligations", "label": "Client Obligations", "kind": "deal", "required": false, "when": "services", "wording_en": "The customer provides site access, a contact person and the information the work needs on time.", "guidance": "What the customer must provide for the work to proceed."}, {"key": "acceptance", "label": "Acceptance", "kind": "deal", "required": false, "when": "services", "wording_en": "The work is accepted when the agreed acceptance criteria are met and the acceptance certificate is signed by both parties.", "guidance": "How and when the delivery is accepted."}, {"key": "payment_terms", "label": "Payment Terms", "kind": "fixed", "required": true, "when": "always", "wording_en": "{payment_terms_text}", "guidance": "Built from the lines: products, services (with milestones), subscriptions, travel; then the invoice due days.", "extend": true}, {"key": "payment_milestones", "label": "Payment Milestones", "kind": "deal", "required": false, "when": "services", "wording_en": "The remaining {service_rest_pct}% is paid on acceptance.", "guidance": "Milestones written into Payment Terms when the project has them.", "inline": true}, {"key": "tax", "label": "Tax", "kind": "fixed", "required": true, "when": "always", "wording_en": "{tax_sentence}", "guidance": "From the lines'' taxes and the quotation''s currency."}, {"key": "order", "label": "How to Order", "kind": "fixed", "required": true, "when": "always", "wording_en": "To order, please send a signed copy of this quotation or a purchase order by email.", "guidance": "Always the last item."}]}', 'json', 'sales', 'Quotation notes template: Services and projects', NULL, false, false, NULL, '841f6464-7707-4f40-a28f-405c7d112b64', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.notes_template.subscription', '{"name": "Subscriptions", "detect": {"sale_models": ["recurring"]}, "clauses": [{"key": "validity", "label": "Validity", "kind": "fixed", "required": true, "when": "always", "wording_en": "This quotation is valid for {validity_days} days from the issue date.", "guidance": "validity_days is the quotation''s own validity (Settings: quotation_validity_days)."}, {"key": "currency", "label": "Currency", "kind": "fixed", "required": true, "when": "always", "wording_en": "All prices are stated in {currency_name} ({currency}) and are net prices.", "guidance": "Filled from the quotation''s currency."}, {"key": "incoterm", "label": "Incoterm and Freight", "kind": "fixed", "required": true, "when": "foreign+goods", "wording_en": "Prices are quoted ex works. Freight and cargo insurance to the delivery site are quoted separately once the order is confirmed.", "guidance": "Quotations in a foreign currency that ship goods. An indicative freight range may be added (notes_deal).", "extend": true}, {"key": "incoterm_remote", "label": "Incoterm and Freight", "kind": "fixed", "required": true, "when": "foreign+no_goods", "wording_en": "Software and services are delivered remotely; no shipment.", "guidance": "Quotations in a foreign currency with nothing to ship."}, {"key": "scope", "label": "Scope", "kind": "deal", "required": true, "when": "always", "wording_en": "{quantity} units of {product} for {purpose}, {inclusions} at {site}.", "guidance": "One or two sentences: what is supplied, how many, where, and what is included. State the assumptions the price rests on. Never name a person; the customer may be named."}, {"key": "price_basis", "label": "Price Basis", "kind": "deal", "required": false, "when": "always", "wording_en": "Prices follow our current price list.", "guidance": "Only when the price differs from a figure the customer saw earlier, or when list prices are discounted. A discount only when the Owner or someone with sales:apply_discount asks for it."}, {"key": "payment_terms", "label": "Payment Terms", "kind": "fixed", "required": true, "when": "always", "wording_en": "{payment_terms_text}", "guidance": "Built from the lines: products, services (with milestones), subscriptions, travel; then the invoice due days.", "extend": true}, {"key": "tax", "label": "Tax", "kind": "fixed", "required": true, "when": "always", "wording_en": "{tax_sentence}", "guidance": "From the lines'' taxes and the quotation''s currency."}, {"key": "non_payment", "label": "Non-payment", "kind": "fixed", "required": true, "when": "always", "wording_en": "Service is paused when an invoice is {non_payment_pause_days} days overdue and may be terminated after {non_payment_terminate_days} days.", "guidance": "Subscriptions only."}, {"key": "order", "label": "How to Order", "kind": "fixed", "required": true, "when": "always", "wording_en": "To order, please send a signed copy of this quotation or a purchase order by email.", "guidance": "Always the last item."}]}', 'json', 'sales', 'Quotation notes template: Subscriptions', NULL, false, false, NULL, '25455de5-6800-47fb-a1fa-01bbfe949bc1', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.notes_families', '{"order": ["subscription", "services", "products"], "fallback": "products"}', 'json', 'sales', 'Quotation notes: template detection order', NULL, false, false, NULL, '84bbbe56-06a7-49ce-a156-7b85854bd474', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.notes.texts', '{"facts": {"travel_skus": ["TRAVEL-*"], "service_family": "services", "logistics_keywords": ["delivery", "shipping", "freight", "installation", "transport"]}, "en": {"currency_names": {"USD": "United States Dollars", "EUR": "Euros", "GBP": "Pounds Sterling", "SGD": "Singapore Dollars", "AUD": "Australian Dollars", "JPY": "Japanese Yen", "CNY": "Chinese Yuan", "MYR": "Malaysian Ringgit"}, "upfront": "100% upfront on purchase order.", "split": "{split_on_order_pct}% on purchase order and {split_rest_pct}% before shipment.", "products_prefix": "Products and licences: ", "services": "{service_down_pct}% down payment on purchase order or contract signing; work begins only after this payment clears. {payment_milestones}", "services_prefix": "Services: ", "milestones_default": "The remaining {service_rest_pct}% is paid on acceptance.", "recurring": "Subscriptions and annual licences are billed yearly in advance.", "travel": "Travel and accommodation: 100% before the trip is booked.", "invoice_due": "Each invoice is payable within {invoice_due_days} days of the invoice date.", "tax_taxed": "Prices include sales tax where shown per line and exclude withholding tax.", "tax_export": "Prices exclude sales tax, import duty, customs clearance and any other taxes or charges in the destination country, which are borne by the buyer.", "year": "year", "years": "years", "range": "{min} to {max}"}}', 'json', 'sales', 'Quotation notes: payment and tax sentences', NULL, false, false, NULL, '8f6ca668-c5b9-4481-a2d7-d25aad7188ce', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.notes.check_phrases', '{"discount": ["discount", "off the quotation total", "price reduction", "rebate"]}', 'json', 'sales', 'Quotation notes: what the check looks for', NULL, false, false, NULL, '77e847ce-d6d5-4424-93b8-d6e33271e836', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.tax_exclusive_note', 'Prices exclude sales tax and withholding tax.', 'string', 'sales', 'Quotation tax: the sentence "Prices exclude tax" adds to the notes', NULL, false, false, NULL, '167c61ad-926e-44c2-b339-9394a3019a8f', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.tax_exclusive_phrases', '["exclude vat", "excludes vat", "excluding vat", "exclusive of vat", "excl. vat", "excl vat", "exclude tax", "excludes tax", "excluding tax", "exclusive of tax", "before tax", "exclude sales tax", "excluding sales tax", "plus vat", "plus tax"]', 'json', 'sales', 'Quotation tax: phrases in the notes that say prices exclude tax (no line may carry tax)', NULL, false, false, NULL, '9929e634-6ca6-4059-ab5a-cdd1eb503365', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('sales.quotation.tax_inclusive_phrases', '["include vat", "includes vat", "including vat", "inclusive of vat", "incl. vat", "incl vat", "include tax", "includes tax", "including tax", "inclusive of tax", "tax included", "vat included", "including sales tax"]', 'json', 'sales', 'Quotation tax: phrases in the notes that say prices include VAT (some line must carry tax)', NULL, false, false, NULL, 'a13d251a-baf4-49c6-8914-c3962e3eff29', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.pricing', '{"claude-haiku-5-5": {"cache_read": 0.01, "cache_write": 0.125, "cache_write_1h": 0.2, "input": 0.1, "output": 0.5}, "claude-opus-4-8": {"cache_read": 0.5, "cache_write": 6.25, "cache_write_1h": 10.0, "input": 5.0, "output": 25.0}, "claude-opus-5": {"cache_read": 0.5, "cache_write": 6.25, "cache_write_1h": 10.0, "input": 5.0, "output": 25.0}, "claude-opus-5-5": {"cache_read": 0.2, "cache_write": 5.0, "cache_write_1h": 8.0, "input": 4.0, "output": 20.0}, "claude-sonnet-5": {"cache_read": 0.2, "cache_write": 2.5, "cache_write_1h": 4.0, "input": 2.0, "output": 10.0}, "claude-sonnet-5-5": {"cache_read": 0.2, "cache_write": 2.5, "cache_write_1h": 4.0, "input": 2.0, "output": 10.0}, "deepseek-chat": {"cache_read": 0, "cache_write": 0, "input": 0.28, "output": 0.42}, "deepseek-reasoner": {"cache_read": 0, "cache_write": 0, "input": 0.28, "output": 0.42}, "gemini-2.0-flash": {"cache_read": 0, "cache_write": 0, "input": 0.1, "output": 0.4}, "gemini-2.5-flash": {"cache_read": 0, "cache_write": 0, "input": 0.3, "output": 2.5}, "gemini-2.5-pro": {"cache_read": 0, "cache_write": 0, "input": 1.25, "output": 10.0}, "gemini-embedding": {"cache_read": 0, "cache_write": 0, "input": 0.15, "output": 0}, "gpt-4.1": {"cache_read": 0, "cache_write": 0, "input": 2.0, "output": 8.0}, "gpt-4.1-mini": {"cache_read": 0, "cache_write": 0, "input": 0.4, "output": 1.6}, "gpt-4.1-nano": {"cache_read": 0, "cache_write": 0, "input": 0.1, "output": 0.4}, "gpt-4o": {"cache_read": 0, "cache_write": 0, "input": 2.5, "output": 10.0}, "gpt-4o-mini": {"cache_read": 0, "cache_write": 0, "input": 0.15, "output": 0.6}, "gpt-5": {"cache_read": 0, "cache_write": 0, "input": 1.25, "output": 10.0}, "gpt-5-mini": {"cache_read": 0, "cache_write": 0, "input": 0.25, "output": 2.0}, "gpt-5-nano": {"cache_read": 0, "cache_write": 0, "input": 0.05, "output": 0.4}, "o3": {"cache_read": 0, "cache_write": 0, "input": 2.0, "output": 8.0}, "o3-mini": {"cache_read": 0, "cache_write": 0, "input": 1.1, "output": 4.4}, "o4-mini": {"cache_read": 0, "cache_write": 0, "input": 1.1, "output": 4.4}}', 'json', 'ai', 'AI prices: model prefix → USD per 1M tokens (input, output, cache read, cache write)', NULL, false, false, NULL, '0491766b-c261-4561-8fbb-b3404e0dfa9c', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('auth.login_max_attempts', '5', 'int', 'system', 'Login: failed attempts allowed per address and account per window', NULL, false, false, NULL, '8095e0a9-cc78-465e-ab13-a9b5e541cfac', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('auth.login_window_seconds', '900', 'int', 'system', 'Login: rate-limit window (seconds)', NULL, false, false, NULL, 'cdb23d06-04c6-4d73-ab68-4993e9b08970', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.autonomy.levels', '{"deal.won": 1, "deal.merge": 1, "email.link": 1, "reply.draft": 0, "lead.promote": 1, "mcp.proposal": 1, "meeting.book": 3, "reminder.add": 1, "activity.void": 1, "contact.merge": 1, "document.send": 1, "identity.link": 1, "mcp.crm_write": 1, "partner.route": 1, "proposal.edit": 1, "proposal.send": 1, "catalog.change": 1, "contact.delete": 1, "email.ack.send": 3, "meeting.cancel": 1, "proforma.raise": 1, "quotation.send": 1, "handoff.kickoff": 1, "lost.reply.send": 1, "proposal.reject": 1, "proposal.revise": 1, "proposal.visual": 1, "chat.held_decide": 1, "email.reply.send": 1, "quotation.cancel": 1, "quotation.update": 1, "assessment.update": 1, "crm.record_change": 1, "price.answer.send": 1, "quotation.reprice": 1, "reengagement.send": 1, "whatsapp.ack.send": 3, "lead.from_whatsapp": 1, "meeting.add_guests": 1, "meeting.slots.send": 1, "organization.merge": 1, "proposal.from_chat": 1, "proposal.quotation": 1, "quotation.revision": 1, "quotation.withdraw": 1, "summary.email.send": 1, "lead.open_returning": 1, "quotation.link_deal": 1, "whatsapp.reply.send": 1, "catalog.product_file": 1, "rates.propose_update": 1, "cadence.followup.send": 1, "contract.create_draft": 1, "meeting.reminder.send": 3, "proposal.create_draft": 1, "proposal.from_sections": 1, "quotation.create_draft": 1, "message.cancel_scheduled": 1, "proposal.discovery.start": 1, "proposal.update_requirements": 1}', 'json', 'nary', 'Nary: autonomy level per action (0 shadow, 1 ask, 2 act and tell, 3 silent)', NULL, false, false, NULL, 'a317f2b5-3584-491c-95fb-5433585b793e', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.autonomy.never_promote', '["quotation.create_draft", "reengagement.send", "discount.apply", "payment.record", "email.outbound_first_touch", "quotation.send", "proforma.raise", "contract.create_draft", "proposal.send", "rates.propose_update", "lead.open_returning", "mcp.crm_write", "mcp.proposal", "organization.merge", "contact.delete", "document.send", "catalog.change", "deal.merge", "activity.void", "quotation.cancel", "chat.held_decide", "deal.won", "catalog.product_file"]', 'json', 'nary', 'Nary: actions that are never promoted past asking', NULL, false, false, NULL, 'e5cb41b3-ec1f-4cb6-a94d-51a67de70d96', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.owner.confirm_twice_actions', '["quotation.create_draft", "quotation.send", "discount.apply", "payment.record", "proforma.raise", "contract.create_draft", "proposal.send", "rates.propose_update", "catalog.change", "catalog.product_file"]', 'json', 'nary', 'Nary and you on WhatsApp: decisions that touch money, confirmed a second time on their final version', NULL, false, false, NULL, 'f0594912-bc30-4310-b893-6b84a84bdf37', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('crm.funnel.lead_sources', '[{"key": "web3forms", "label": "Website form (Web3forms email)"}, {"key": "inline_web3forms", "label": "Website inline form (Web3forms email)"}, {"key": "whatsapp_direct", "label": "WhatsApp (direct, no form)"}, {"key": "whatsapp", "label": "WhatsApp"}, {"key": "email", "label": "Email"}, {"key": "website_form", "label": "Website form"}, {"key": "linkedin", "label": "LinkedIn"}, {"key": "founder_network", "label": "Founder network"}, {"key": "bizdev_network", "label": "BizDev network"}, {"key": "partner_referral", "label": "Partner referral"}, {"key": "referral", "label": "Referral"}, {"key": "cold_outreach", "label": "Cold outreach"}, {"key": "event", "label": "Event"}, {"key": "other", "label": "Other"}, {"key": "website_inline_form", "label": "Website inline form"}]', 'json', 'crm', 'Funnel: lead sources', NULL, false, false, NULL, 'b4e7edf8-be63-41b2-af71-55751ccbce3a', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('nary.web3forms.known_fields', '["name", "email", "phone", "company", "interested_in", "message", "from_page", "submission_source", "timezone", "landing_page", "first_referrer", "request_type", "submission_id"]', 'json', 'nary', 'Web3forms: the fields the form sends (another field → format-drift warning, still stored)', NULL, false, false, NULL, 'e6eca60b-0000-44c6-a33a-a06b98f19ca4', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('ai.routing', '{"assistant": {"effort": "high", "max_tokens": 16000, "model": "claude-opus-5-5", "provider": "anthropic", "cache_ttl": "1h"}, "crm_ai_field": {"effort": "low", "max_tokens": 2000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "crm_enrichment": {"effort": "medium", "max_tokens": 4000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "crm_research": {"effort": "medium", "max_tokens": 8000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "default": {"effort": "medium", "max_tokens": 8000, "model": "claude-opus-5-5", "provider": "anthropic"}, "doc_intel": {"effort": "medium", "max_tokens": 8000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "embedding": {"effort": null, "max_tokens": null, "model": "gemini-embedding-001", "provider": "gemini"}, "nary.brief": {"effort": "medium", "max_tokens": 8000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.card": {"effort": "medium", "max_tokens": 4000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.cascade_review": {"effort": "medium", "max_tokens": 2000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "nary.owner_chat": {"cache_ttl": "1h", "effort": "high", "max_tokens": 16000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.classify": {"effort": "low", "max_tokens": 1000, "model": "claude-haiku-5-5", "provider": "anthropic"}, "nary.cost_estimate": {"effort": "medium", "max_tokens": 6000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "nary.fit_check": {"effort": "medium", "max_tokens": 4000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "nary.followup_draft": {"effort": "medium", "max_tokens": 3000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.handoff": {"effort": "high", "max_tokens": 8000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.history_classify": {"effort": "low", "max_tokens": 1000, "model": "claude-haiku-5-5", "provider": "anthropic"}, "nary.history_read": {"effort": "medium", "max_tokens": 4000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "nary.identity_match": {"effort": "low", "max_tokens": 1000, "model": "claude-haiku-5-5", "provider": "anthropic"}, "nary.lost_reply": {"effort": "medium", "max_tokens": 2000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.meeting_brief": {"effort": "medium", "max_tokens": 3000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.memory": {"effort": "medium", "max_tokens": 4000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "nary.negotiation": {"effort": "medium", "max_tokens": 4000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.proposal_draft": {"effort": "high", "max_tokens": 16000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.proposal_edit": {"effort": "medium", "max_tokens": 12000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.quote_lines": {"effort": "low", "max_tokens": 2000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "nary.rate_research": {"effort": "medium", "max_tokens": 4000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "nary.reply_alert": {"effort": "low", "max_tokens": 400, "model": "claude-haiku-5-5", "provider": "anthropic"}, "nary.reply_draft": {"effort": "high", "max_tokens": 4000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.report": {"effort": "medium", "max_tokens": 6000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.score": {"effort": "low", "max_tokens": 1500, "model": "claude-haiku-5-5", "provider": "anthropic"}, "nary.slot_pick": {"effort": "low", "max_tokens": 500, "model": "claude-haiku-5-5", "provider": "anthropic"}, "nary.sop_revision": {"effort": "high", "max_tokens": 16000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.summary_email": {"effort": "medium", "max_tokens": 4000, "model": "claude-opus-5-5", "provider": "anthropic"}, "nary.transcript_extract": {"effort": "medium", "max_tokens": 6000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "nary.triage": {"effort": "low", "max_tokens": 1000, "model": "claude-haiku-5-5", "provider": "anthropic"}, "nary.vision": {"effort": "low", "max_tokens": 2000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "nary.web3forms_extract": {"effort": "low", "max_tokens": 2000, "model": "claude-haiku-5-5", "provider": "anthropic"}, "pdf_import": {"effort": "low", "max_tokens": 32000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "record_brief": {"effort": "medium", "max_tokens": 4000, "model": "claude-sonnet-5-5", "provider": "anthropic"}, "scoring": {"effort": "low", "max_tokens": 2000, "model": "claude-haiku-5-5", "provider": "anthropic"}, "suggestion": {"effort": "medium", "max_tokens": 4000, "model": "claude-opus-5-5", "provider": "anthropic"}, "web_search": {"effort": null, "max_tokens": null, "model": "gemini-2.0-flash", "provider": "gemini"}, "nary.document_read": {"model": "claude-sonnet-5-5", "effort": "low", "provider": "anthropic", "max_tokens": 4000}, "nary.visual_check": {"model": "claude-sonnet-5-5", "effort": "low", "provider": "anthropic", "max_tokens": 1500}, "nary.visual_spec": {"model": "claude-sonnet-5-5", "effort": "medium", "provider": "anthropic", "max_tokens": 8000}}', 'json', 'ai', 'AI routing: job → provider, model, effort, max tokens', NULL, false, false, NULL, '8dd50ae3-9228-41af-8017-de8a6681c5b4', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('partner.training.max_attempts', '5', 'int', 'partner', 'Partner training: quiz submissions allowed per module', NULL, false, false, NULL, 'a0a82acc-d4ae-469c-9ad4-6ea69b11e62b', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('public.forms.requests_per_minute', '10', 'int', 'system', 'Public forms: submissions per minute from one address (support tickets, portal sign-ups, call waiting room)', NULL, false, false, NULL, '7e3fdf99-2ea4-46de-8d09-c53d01650ba3', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('calls.guest_wait.max_waiting', '50', 'int', 'calls', 'Video calls: most guests waiting to be admitted to one room', NULL, false, false, NULL, '98212f3c-9bef-4dd9-8722-c474df5cb809', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('calls.max_participants', '10', 'int', 'calls', 'Video calls: most people in one call', NULL, false, false, NULL, 'd2fca93b-f624-4d7b-9be1-8bbab1c48511', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('calls.stun_urls', '[]', 'json', 'calls', 'Video calls: extra STUN servers (JSON list of stun: URLs); empty uses only this installation''s own TURN/STUN', NULL, false, false, NULL, '5edee9ce-2f93-4ea6-a546-a08847d76269', now(), now());
INSERT INTO public.system_settings (key, value, value_type, category, label, description, is_public, is_encrypted, updated_by_id, id, created_at, updated_at) VALUES ('vendor.portal.max_upload_mb', '10', 'int', 'vendor', 'Vendor portal: largest document a vendor may upload (MB)', NULL, false, false, NULL, '1a0db5c1-e781-4d39-8a50-665bbed79306', now(), now());
