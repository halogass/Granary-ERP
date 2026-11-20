# Manufacturing and Inventory

<!-- nav:top -->
[Home](Home) › Using Granary › Buy and make › **Manufacturing and Inventory**

**On this page:** [AI in this module](#ai-in-this-module) · [Find your way around](#find-your-way-around) · [Products and bills of materials](#products-and-bills-of-materials) · [Production orders](#production-orders) · [Unit registry](#unit-registry) · [Quality control](#quality-control) · [Inventory](#inventory) · [Warehouses](#warehouses) · [Stock movements](#stock-movements) · [Warranty and RMA](#warranty-and-rma) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

The Manufacturing and Inventory module is for companies that build, assemble or stock physical products. You
define products and their bills of materials (BOMs), run production orders that reserve and consume
components, inspect finished units, give every unit a serial number and follow it to the customer, and handle
warranty claims and returns (RMA). Inventory keeps component stock levels, warehouses with their storage
locations, and every stock movement. Production staff, warehouse staff and after-sales support use it.

![The unit serial registry with units at different stages](images/manufacturing-units.png)

---

## AI in this module

Nary has no manufacturing or inventory tools yet: it can't read or change bills of materials, production, stock or warranty records. In the chat it can explain how this module works, from the ERP guide built into Granary. See [AI Across Granary](AI-Across-Granary) for what the AI does in the other modules. (Nary does read and, on the Owner's card, change the sales catalog: see [Sales](Sales).)

---

## Find your way around

Open **Manufacturing** in the sidebar. It has these pages:

| Page | What it is for |
|---|---|
| **Products / BOMs** | The products you make, and the bill of materials for each |
| **Production Orders** | Manufacturing runs, from planning to completion |
| **Unit Registry** | Every unit by serial number, from production to the customer |
| **QC Records** | Inspection records for units |
| **Inventory** | Component stock, reorder points and adjustments |
| **Warranty / RMA** | Warranties for shipped units and return cases |
| **Warehouses** | Warehouses and their locations (zone, aisle, rack, bin) |
| **Stock Movements** | Receipts, issues, transfers and adjustments of stock |

---

## Products and bills of materials

### Add a product

1. Open **Manufacturing > Products / BOMs** and click **New product**.
2. Enter the **Product code** (for example `KT-DESK-01`) and the **Name** (for example "Standing desk frame").
3. Pick the **Category**: Finished good, Assembly, Peripheral, Component or Other.
4. Optional: the **Product line** (from [R&D](RnD)), a **Description**, the **Unit cost** and its **Currency**.
5. Enter a **Serial prefix** of 2 to 6 letters, for example `KTD`. Granary uses it to generate the serial of
   every unit of this product.
6. Click **Create product**.

You can change a product's line from the list with the **Set line...** picker. The tabs are **All products**,
**Active** and **Inactive**.

### Define the bill of materials

![Defining a bill of materials for a product](images/manufacturing-bom.png)

1. On the product's row, click **BOM**.
2. For each component, enter the **Component name**, the **SKU**, the **Quantity** and the **Unit** (for example
   `pcs`). Click **Add component** for more.
3. Click **Save BOM**.

Each component needs a name and a quantity. A new BOM is a draft and goes to its approval chain (see
[Approvals and Signing](Approvals-and-Signing)). Production uses only an approved BOM. The product's details
show its BOM: the latest approved version, or the newest one while none is approved.

Use the component SKUs that you track in **Inventory**, so production can reserve and consume them.

---

## Production orders

### Plan an order

1. Open **Manufacturing > Production Orders** and click **New order**.
2. Pick the **Product**. Its BOM is selected for you in **Bill of materials**.
3. Enter the **Quantity**, the **Due date** and optional **Notes**.
4. Click **Create order**. It starts as **Planned**.

### Move the order through production

Each row has a button for the next step:

| Status | What happens when you move to it |
|---|---|
| Planned | The order is created |
| Component check | You check that the components are there |
| In production | Granary reserves the components the BOM needs for the quantity. A BOM must be linked |
| QC | The units are inspected |
| Completed | Granary consumes the reserved components and books the finished goods into stock |

An order can go back from Component check to Planned, and from QC to In production. It can be cancelled at
any step before completion; cancelling or going back to Planned releases the reserved components.

Completing fails with "Not enough components on hand to complete" when stock is short. Record the missing
stock first (see [Stock movements](#stock-movements)).

Click **Cost** on a row to enter the **actual material cost**; the list shows it next to the BOM cost. Only
planned or cancelled orders can be deleted. The tabs are **All orders**, **Planned**, **In production**,
**QC** and **Completed**.

---

## Unit registry

Every unit you build gets its own serial number, so you can follow it through production, QC, stock and
shipping, and find it again for warranty claims.

### Register a unit

1. Open **Manufacturing > Unit Registry** and click **Register unit**.
2. Pick the **Production order** and the **Product**.
3. Optional: the **Hardware revision** (for example `v1.2`) and **Notes**.
4. Turn on **Test unit** for a unit that will never go to a customer.
5. Click **Register unit**.

Granary generates the serial from the product's serial prefix, for example `KTD-7K2M9-QXH48`. A test unit's
serial has a `T` after the prefix (for example `KTDT-...`). The **Unit registered** box shows the serial with a
copy button: write it on the unit's label.

The last character of a generated serial is a check character. When you search, case, dashes and spaces don't
matter, and `I`/`L` are read as `1` and `O` as `0`. A mistyped serial is caught: "This serial's check
character does not match: probably a typo".

### Move a unit through its life

Open a unit's details and use the **Move to:** buttons:

| From | Can move to |
|---|---|
| In production | QC pending |
| QC pending | QC passed, QC failed |
| QC failed | Rework, QC pending |
| Rework | QC pending |
| QC passed | In stock |
| In stock | Reserved, Shipped |
| Reserved | Shipped, In stock |
| Shipped | Returned |

Logging an inspection (see [Quality control](#quality-control)) sets QC passed or QC failed for you.

### Shipping a unit

When a unit leaves for the customer, move it to **Shipped**. The unit's details show the **Delivery date**, the
**Tracking no.** and the **Warranty expiry** when they are recorded. Then register its warranty (see
[Warranty and RMA](#warranty-and-rma)).

The tabs are **All units**, **In production**, **QC pending**, **In stock** and **Shipped**. You can filter by
product and search by serial.

---

## Quality control

![Logging a QC inspection with its checklist](images/manufacturing-qc-log.png)

1. Open **Manufacturing > QC Records** and click **Log inspection**.
2. Pick the **Unit** by its serial.
3. Work through the **Checklist**. It starts with Power on test, Function test, Burn-in test and Enclosure &
   cosmetics. Mark each check, or type a **New check** and click **Add**.
4. Choose the **Overall result**: Pass, Fail or Passed with conditions.
5. For **Passed with conditions**, write the conditions (what must be true for this unit to be used). They are
   required, and the unit counts as passed.
6. Click **Log inspection**.

A pass moves the unit to QC passed, a fail to QC failed. The tiles show the total, passes, fails and the pass
rate. The tabs are **All**, **Pass**, **Fail** and **Conditional**.

---

## Inventory

**Manufacturing > Inventory** holds the stock of components (and other items you track by SKU).

### Add a component

1. Click **Add component**.
2. Enter the **Name**, the **SKU**, the **Qty on hand**, the **Reorder point** and the **Unit**.
3. Optional: the **Category**, **Unit cost** and **Currency**, **Location** and **Supplier**.
4. Click **Add component**.

A SKU can exist only once. The **Low stock** tab lists items at or below their reorder point. The tiles show
the number of components, low-stock items, total SKUs and the stock value.

### Adjust stock

1. On the item's row, click **Adjust**.
2. Enter the **Adjustment (+/-)**, for example `+50` or `-10`, and a **Reason**.
3. Click **Apply adjustment**.

An adjustment that would make the stock negative is refused.

Stock also changes on its own: goods receipts add to it (see [Procurement](Procurement)), production orders
reserve and consume it, and completed orders add finished goods.

---

## Warehouses

![A warehouse with its zones, aisles, racks and bins](images/manufacturing-warehouses.png)

### Add a warehouse

1. Open **Manufacturing > Warehouses** and click **New warehouse**.
2. Enter the **Name** (for example "Tokyo Main Warehouse"), the **Code** (for example `WH01`) and the
   **Address**.
3. Tick **Set as default warehouse** if it is your main one.
4. Click **Create warehouse**.

A warehouse can be deactivated, except the default warehouse.

### Add storage locations

Locations follow a hierarchy: **zone > aisle > rack > bin**.

1. Select the warehouse and click **Add location**.
2. Enter the **Name** (for example "Zone A") and the **Code** (for example `ZA`).
3. Pick the **Type**: Zone (top-level area), Aisle, Rack or Bin (storage unit).
4. Optional: pick the **Parent location**. Leave it empty for a top-level location.
5. Click **Add location**.

---

## Stock movements

**Manufacturing > Stock Movements** is the log of every movement, and where you record one by hand.

1. Click **Record move**.
2. Pick the **Movement type**:
   - **Receipt (stock in)**: stock arrives at a location;
   - **Issue (stock out)**: stock leaves a location;
   - **Internal transfer (location to location)**: stock moves between two locations;
   - **Adjustment (manual correction)**.
3. Enter the **Component SKU** and the **Quantity**. The **Component name** fills in from inventory when you
   leave it empty.
4. Pick the **From location** and/or the **To location**. A transfer needs both.
5. Add a **Reference** (for example a PO or work order number) and **Notes**.
6. Click **Record move**.

The quantity must be above zero (an adjustment can't be zero). A move or transfer that would take more than
is on hand is refused. A transfer is recorded as two lines, one out and one in.

The tabs are **All moves**, **Receipts**, **Issues**, **Transfers** and **Adjustments**; you can filter by SKU.

![Recording an internal transfer between two locations](images/manufacturing-stock-transfer.png)

---

## Warranty and RMA

### Register a warranty

1. Open **Manufacturing > Warranty / RMA** and click **Register warranty**.
2. Enter the unit's **Serial number** (the unit must be in the unit registry) and pick the **Product**.
3. Enter the **Customer name** and **Customer email**, the **Purchase date** and the **Warranty expiry**.
4. Click **Register**.

Active warranties that end within 30 days are marked **Expiring soon**.

### Handle a return (RMA)

1. On the warranty's row, click **Open RMA**.
2. Describe the problem in **Issue description** and click **Open RMA**. Granary creates an RMA case with its
   own number.
3. When the case is resolved, click **Close RMA**.

The tabs are **All**, **Active**, **RMA open**, **RMA closed** and **Expired**.

---

## Settings that affect this module

Open **Settings > General**, then **Operations > Manufacturing**.

| Setting | Key | What it does |
|---|---|---|
| Serial generation tries | `manufacturing.serial.generate_tries` | How many times Granary draws a new serial when a generated one is already taken (rare) |

Approval chains for BOMs are set under **Approvals & signing**; see
[Approvals and Signing](Approvals-and-Signing). The over-delivery tolerance on goods receipts is a procurement
setting; see [Procurement](Procurement).

---

## Permissions

| Permission | Allows |
|---|---|
| `manufacturing:view_products` | See products, BOMs, production orders, units, QC records and warranties |
| `manufacturing:manage_bom` | Create and change products and their BOMs |
| `manufacturing:create_production_order` | Create production orders |
| `manufacturing:manage_production` | Move production orders, set their cost, delete them; register and move units |
| `manufacturing:log_qc` | Log QC inspections |
| `manufacturing:view_inventory` | See component inventory |
| `manufacturing:manage_inventory` | Add components and adjust stock |
| `manufacturing:manage_warranty` | Register warranties, open and close RMAs |
| `inventory:view` | See warehouses, locations and stock movements |
| `inventory:manage` | Create and change warehouses and locations, record stock movements and transfers |

Portal users never reach this module. Superadmins can do everything.

---

## Related pages

- [Procurement](Procurement): purchase orders and goods receipts that bring components in
- [R&D](RnD): product lines
- [Approvals and Signing](Approvals-and-Signing): BOM approval
- [Sales](Sales): the product catalog you sell from
- [Operations](Operations): equipment maintenance for your production machines

<!-- nav:bottom -->
---

← Previous: [Procurement](Procurement) | Next: [Operations](Operations) →
<!-- /nav:bottom -->
