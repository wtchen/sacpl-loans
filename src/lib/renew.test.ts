// Unit tests for the pure renewal logic (button eligibility and the fake
// renewal used for debug books). Dates are constructed in local time.
import { describe, expect, it } from "vitest";
import { isRenewBlocked, simulatedRenewalDue } from "./renew";

// 6 Sep 2026, 22:15 local time.
const NOW = new Date(2026, 8, 6, 22, 15).getTime();

const idle = {
  busy: false,
  reconnecting: false,
  hasCache: false,
  lastFetchAt: 0,
};

describe("isRenewBlocked", () => {
  it("allows renewing in the normal signed-in state", () => {
    expect(isRenewBlocked({ ...idle, hasCache: true, lastFetchAt: NOW })).toBe(false);
    expect(isRenewBlocked(idle)).toBe(false);
  });

  it("blocks while a refresh/login request is in flight", () => {
    expect(isRenewBlocked({ ...idle, busy: true })).toBe(true);
  });

  it("blocks while the backend is re-authenticating", () => {
    expect(isRenewBlocked({ ...idle, reconnecting: true })).toBe(true);
  });

  it("blocks while only the disk cache is on screen", () => {
    expect(isRenewBlocked({ ...idle, hasCache: true })).toBe(true);
    expect(isRenewBlocked({ ...idle, hasCache: true, lastFetchAt: 0 })).toBe(true);
  });

  it("allows renewing once a fresh fetch is known", () => {
    expect(isRenewBlocked({ ...idle, hasCache: true, lastFetchAt: 1 })).toBe(false);
  });

  it("blocks when several conditions combine", () => {
    expect(
      isRenewBlocked({ busy: true, reconnecting: true, hasCache: true, lastFetchAt: 0 })
    ).toBe(true);
  });
});

describe("simulatedRenewalDue", () => {
  it("moves the due date three weeks ahead", () => {
    expect(simulatedRenewalDue(NOW, "en-US")).toBe("Due Sep 27, 2026");
  });

  it("crosses month and year boundaries", () => {
    const lateAugust = new Date(2026, 7, 20, 9, 0).getTime();
    expect(simulatedRenewalDue(lateAugust, "en-US")).toBe("Due Sep 10, 2026");
    const lateDecember = new Date(2026, 11, 25, 9, 0).getTime();
    expect(simulatedRenewalDue(lateDecember, "en-US")).toBe("Due Jan 15, 2027");
  });
});