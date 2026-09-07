// Unit tests for the refresh-time helpers (footer "Updated …" label and the
// stale-on-open refresh decision). All dates are constructed in local time so
// the day-boundary arithmetic matches the app's behavior.
import { describe, expect, it } from "vitest";
import { DAY_MS, fullStamp, isStale, stampDate, startOfDay, updatedLabel } from "./updated-label";

// 6 Sep 2026, 22:15 local time.
const NOW = new Date(2026, 8, 6, 22, 15).getTime();
const HOUR_MS = 60 * 60 * 1000;
const fixedTime = () => "10:15:33 PM";

describe("startOfDay", () => {
  it("collapses instants from the same day", () => {
    const midnight = new Date(2026, 8, 6, 0, 0, 0, 0).getTime();
    expect(startOfDay(new Date(2026, 8, 6, 0, 0, 0, 1).getTime())).toBe(midnight);
    expect(startOfDay(new Date(2026, 8, 6, 23, 59).getTime())).toBe(midnight);
    expect(startOfDay(new Date(2026, 8, 7, 0, 30).getTime())).toBe(midnight + DAY_MS);
  });
});

describe("stampDate", () => {
  it("formats month-first and omits the year within the same year", () => {
    expect(stampDate(NOW, NOW, "en-US")).toBe("Sep 6");
    expect(stampDate(new Date(2026, 11, 31, 9, 5).getTime(), NOW, "en-US")).toBe("Dec 31");
  });

  it("shows the year for a previous calendar year", () => {
    expect(stampDate(new Date(2025, 8, 6, 10, 0).getTime(), NOW, "en-US")).toBe("Sep 6 2025");
  });
});

describe("fullStamp", () => {
  it("formats time per the locale (12/24-hour clock)", () => {
    expect(fullStamp(NOW, NOW, "en-US")).toBe("Sep 6 10:15 PM");
    expect(fullStamp(new Date(2026, 8, 6, 5, 7).getTime(), NOW, "en-US")).toBe("Sep 6 5:07 AM");
  });

  it("keeps the year for previous calendar years", () => {
    expect(fullStamp(new Date(2025, 8, 6, 10, 0).getTime(), NOW, "en-US")).toBe("Sep 6 2025 10:00 AM");
  });
});

describe("updatedLabel", () => {
  it("is empty when never refreshed", () => {
    expect(updatedLabel(0, NOW)).toBe("");
  });

  it("shows the time for a same-day refresh", () => {
    const threeHoursAgo = NOW - 3 * HOUR_MS;
    expect(updatedLabel(threeHoursAgo, NOW, fixedTime)).toBe("Updated 10:15:33 PM");
  });

  it("shows yesterday across a day boundary", () => {
    expect(updatedLabel(NOW - DAY_MS, NOW, fixedTime)).toBe("Updated yesterday");
  });

  it("counts yesterday from just past midnight", () => {
    const justAfterMidnight = new Date(2026, 8, 7, 0, 30).getTime();
    const lateLastNight = new Date(2026, 8, 6, 23, 10).getTime();
    expect(updatedLabel(lateLastNight, justAfterMidnight, fixedTime)).toBe("Updated yesterday");
  });

  it("shows day counts from 2 to 6", () => {
    expect(updatedLabel(NOW - 2 * DAY_MS, NOW, fixedTime)).toBe("Updated 2 days ago");
    expect(updatedLabel(NOW - 6 * DAY_MS, NOW, fixedTime)).toBe("Updated 6 days ago");
  });

  it("switches to the date form after 6 days", () => {
    expect(updatedLabel(NOW - 7 * DAY_MS, NOW, fixedTime, "en-US")).toBe("Updated Aug 30");
    expect(updatedLabel(NOW - 20 * DAY_MS, NOW, fixedTime, "en-US")).toBe("Updated Aug 17");
  });

  it("keeps the year in the date form for previous calendar years", () => {
    const lastYear = new Date(2025, 11, 20, 18, 0).getTime();
    expect(updatedLabel(lastYear, NOW, fixedTime, "en-US")).toBe("Updated Dec 20 2025");
  });
});

describe("isStale", () => {
  it("treats a missing timestamp as stale", () => {
    expect(isStale(0, NOW, HOUR_MS)).toBe(true);
  });

  it("compares age against the interval", () => {
    expect(isStale(NOW - 30 * 60 * 1000, NOW, HOUR_MS)).toBe(false);
    expect(isStale(NOW - 3 * HOUR_MS, NOW, HOUR_MS)).toBe(true);
  });

  it("is false exactly at the interval boundary", () => {
    expect(isStale(NOW - HOUR_MS, NOW, HOUR_MS)).toBe(false);
  });
});
