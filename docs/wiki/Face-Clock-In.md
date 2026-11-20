# Face Clock-In

<!-- nav:top -->
[Home](Home) › Using Granary › People › **Face Clock-In**

**On this page:** [AI in this module](#ai-in-this-module) · [How it works](#how-it-works) · [Before you start (HR)](#before-you-start-hr) · [Clock in on your own device](#clock-in-on-your-own-device) · [Clock in at a kiosk](#clock-in-at-a-kiosk) · [Review weak matches (HR)](#review-weak-matches-hr) · [Settings that affect this module](#settings-that-affect-this-module) · [Permissions](#permissions)
<!-- /nav:top -->

Face clock-in lets employees clock in and out with their face, either on their own phone or laptop, or on a
registered kiosk tablet at the office. Granary checks that a live person is in front of the camera (liveness),
matches the face, and checks the location against your office geofence. Face matching runs on Granary's own
`face` service: no outside face API and no API key. HR enrolls faces, registers kiosks and reviews the
clock-ins the system could not confirm. The resulting check-in and check-out times feed attendance and
payroll (see [HR and Payroll](HR-and-Payroll)).

![Personal clock-in on a phone, during the face check](images/face-personal-clock.png)

---

## AI in this module

Face matching and liveness run on Granary's own `face` service, not on Nary or an outside AI provider, and they need no AI key or budget. Nary has no clock-in tools. See [AI Across Granary](AI-Across-Granary).

---

## How it works

There are two ways to clock in. They trust the device differently.

| | Personal device | Kiosk tablet |
|---|---|---|
| URL | `https://your-domain/clock` | `https://your-domain/clock?kiosk=1` |
| Who signs in | The employee | Nobody: the tablet is registered once by HR |
| Face matched against | The signed-in employee only | Every enrolled employee |
| Who does the face work | Granary's server, from three frames the phone uploads | The tablet, then the server checks the result again |
| Location | The phone's GPS must be inside an office geofence | The kiosk's office |

In both modes:

- **Liveness**: first a passive check, where the screen flashes a few soft colors and a live face reflects
  them. If that check is not sure, the employee is asked to turn their head left or right and then face the
  camera again. You can switch the passive check off, so the head turn is always used.
- **No photos are kept.** Granary stores only face descriptors (lists of numbers) from the enrollment photos,
  and never stores the frames from a clock-in.
- Nothing face-related (the engine, its model files, the descriptors) is sent to a browser before it has
  signed in or authenticated as a kiosk.

---

## Before you start (HR)

1. Add your office locations, with a geofence. See [Office locations](#office-locations).
2. Link each employee's user account to their employee record (in **HR > Employees**). A user without a linked
   employee record cannot clock in.
3. Enroll each employee's face. See [Enroll a face](#enroll-a-face).
4. Optional: register kiosk tablets. See [Register a kiosk tablet](#register-a-kiosk-tablet).

### Office locations

1. Open **Settings > General**, area **People & payroll**, section **Office locations**.
2. Click **Add location**.
3. Enter a **Name** (for example "Head office"), the **Address**, the **Latitude** and **Longitude** of the
   office, and a **Geofence radius** in meters. Employees must be within the radius to clock in; 100 to 300 m
   works well.
4. Save.

Personal clock-in is refused until at least one office location exists.

### Enroll a face

1. Open **HR > Employees** and click **Face ID** on the employee.
2. Click **Upload face photos** and choose 1 to 4 clear photos of this person alone: good light, face
   centered, close to the camera.
3. Click **Enroll N photos**.

The dialog then shows **Face enrolled** with the date. To replace the photos, use **Re-enroll with new
photos**. To remove the enrollment, click **Remove face enrollment**.

Granary refuses a photo when:

- it shows no face, or more than one face;
- the face is too small or unclear;
- the photos do not look like the same person;
- the face is already enrolled for another employee.

Each photo can be up to 8 MB. Enrolling and removing faces needs `hr:manage_employees`.

![Face enrollment dialog on an employee](images/face-enrollment.png)

### Register a kiosk tablet

1. On the tablet, open `https://your-domain/clock?kiosk=1`. It shows **This device isn't registered as a
   kiosk**.
2. Click **Sign in as HR** and sign in with an account that holds `hr:manage_attendance`.
3. Enter a **Tablet name** (for example "Lobby tablet"), pick the **Office**, and register.
4. Sign out of your HR account on the tablet. The tablet keeps its own device token and opens a kiosk session
   that it renews by itself.

To see or revoke kiosks, open **Settings > General**, area **People & payroll**, section **Kiosk tablets**. The
list shows each **Tablet**, its **Office**, its token prefix and when it was **Last seen**. Revoking a kiosk
signs it out at once; to use it again, register it again.

---

## Clock in on your own device

1. Click **Clock in** in the top bar of Granary (it changes to **Clock out** once you are in), or open
   `/clock` on your phone and sign in.
2. Pick **Clock in** or **Clock out**.
3. Allow location access when the browser asks.
4. Allow camera access, look at the screen and hold still. The screen may flash a few colors.
5. If asked, follow the instruction (for example **Turn left**), then face the camera again.

![Clocking in with the head-turn check](images/face-personal-clock.gif)

What can go wrong, and what to do:

| Message | What it means |
|---|---|
| Location is required | Allow location access in the browser and try again |
| Location is too imprecise | The GPS accuracy is worse than the **Largest GPS inaccuracy** setting. Wait a moment, or move near a window |
| You are N m from (office), you must be within N m | You are outside every office geofence |
| No office locations are configured | HR has not added an office yet |
| Your face is not enrolled yet | Ask HR to enroll you |
| Your account is not linked to an employee record | Ask HR to link your user to your employee record |
| Face does not match your account | The face is not the signed-in employee |
| HR has been asked to review | The match was just weaker than required. HR approves or rejects it |
| The check timed out | A step took longer than allowed. Try again |

Granary limits how many checks a person can start in a minute, so retrying very fast is refused for a short
time.

---

## Clock in at a kiosk

1. Stand alone in front of the tablet.
2. Tap **Clock in** or **Clock out**.
3. Look at the screen and hold still, then follow the instruction on screen.

The result stays on screen for a few seconds:

- Success: your name and the time.
- **Not recognised**: try again, or ask HR to record your attendance.
- **Only one person, please**: more than one face was in view.
- Pending (orange): the match was not certain enough; HR has been asked to review.

The tablet matches the face itself, then the server runs its own search on the result. The server accepts
the clock-in only if its best match is the same employee the tablet named. If the server matches someone
else, the clock-in is refused with "Identity could not be confirmed" and HR is alerted.

---

## Review weak matches (HR)

Open **HR > Attendance**. The page has the tabs **Records**, **Clock events**, **Pending overrides** and
**Attempts**.

![Pending overrides on the Attendance page](images/face-pending-overrides.png)

**Pending overrides** lists clock-ins the system could not confirm: weak matches, unrecognized faces, failed
liveness and kiosk mismatches. A face match slightly weaker than the **Face match strictness** setting (up to
0.10 above it) goes here instead of being refused outright, and HR gets a notification.

1. Click **Review** on a row. The **Review clock event** dialog shows the employee, recognition confidence, distance and
   liveness result.
2. To accept it, pick the employee in **Assign to employee** if needed, write **Override notes**, and click
   **Approve**. Granary records the check-in or check-out.
3. To refuse it, write a note and click **Reject**.

**Clock events** lists every clock event with its **Event**, **Status** (for example Matched, Low confidence,
Liveness failed, Kiosk mismatch, Overridden by HR), **Recognition**, **Liveness** and **Kiosk**.

**Attempts** shows every face check from kiosks and personal devices over the last days and how it ended: by
outcome, by step reached, by device, why the passive check fell back to the head turn, and timeout rates. Use
it to tune the settings below. No photos are kept.

Approving and rejecting needs `hr:manage_attendance`. The **Attempts** summary needs `hr:view_attendance`.

---

## Settings that affect this module

Open **Settings > General**, area **People & payroll**, section **Attendance**.

| Setting | Key | What it does |
|---|---|---|
| Face match strictness | `hr.face_match_distance` | Largest face distance accepted: 0.35 strict to 0.60 lenient (default 0.50). Slightly weaker matches go to HR review |
| Largest GPS inaccuracy | `hr.geo_max_accuracy_m` | Personal-device clock-ins with a less precise location are refused (default 100 m) |
| Work start time | `hr.work_start_time` | Clock-ins after this time are flagged late |
| Overnight shift window | `hr.overnight_shift_hours` | A clock-out after midnight closes yesterday's shift if it began within this many hours |
| Face check time per step | `hr.face.step_timeouts_ms` | Milliseconds for each step of the head-turn check: find, action, return |
| Phone check valid for | `hr.face.challenge_ttl_seconds` | How long a phone's face check stays valid |
| Passive face check first | `hr.face.passive.enabled` | Color flash first, head turn as the fallback. Off: always the head turn |
| Colours per check | `hr.face.passive.flash_count` | How many colors the screen flashes, 4 to 6 |
| Time per colour | `hr.face.passive.flash_ms` | At least 250 ms, so the screen flashes fewer than 3 times a second |
| Photo after | `hr.face.passive.settle_ms` | When the frame is taken after each color comes on |
| Flash colours | `hr.face.passive.palette` | The colors the check picks from, as [r, g, b] lists |
| Smallest colour response | `hr.face.passive.flash_min_delta` | How much the face color must change |
| Colour match needed | `hr.face.passive.flash_min_corr` | How closely the face color must follow the flashed colors, 0 to 1 |
| Real face score needed | `hr.face.passive.spoof_min_real` | Smallest anti-spoof score that the face is real, not a photo or screen |

The sections **Office locations** and **Kiosk tablets** sit in the same area.

Server side, the backend reaches the face service at `FACE_SERVICE_URL` (default `http://face:3003`) with
`FACE_SERVICE_TOKEN`. `FACE_MAX_CONCURRENT` sets how many analyses run at once. See `docs/CONFIGURATION.md`
in the repository.

---

## Permissions

| Permission | Allows |
|---|---|
| `hr:manage_employees` | Enroll, re-enroll and remove faces; see any employee's enrollment status |
| `hr:manage_attendance` | Register and revoke kiosks; approve or reject pending clock events |
| `hr:view_attendance` | See everyone's attendance, clock events and the face check attempts summary |

Clocking in on your own device needs only a signed-in internal user linked to an employee record with an
enrolled face.

---

## Related pages

- [HR and Payroll](HR-and-Payroll): attendance records, late and overtime, payroll
- [Collaboration](Collaboration)

<!-- nav:bottom -->
---

← Previous: [HR and Payroll](HR-and-Payroll) | Next: [Projects](Projects) →
<!-- /nav:bottom -->
