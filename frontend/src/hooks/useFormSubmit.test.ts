import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../services/apiClient";
import { useFormSubmit } from "./useFormSubmit";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("useFormSubmit", () => {
  it("is loading while the action runs", async () => {
    let finish = () => {};
    const action = () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      });

    const { result } = renderHook(() => useFormSubmit(action));
    expect(result.current.loading).toBe(false);

    act(() => {
      void result.current.submit();
    });
    await waitFor(() => {
      expect(result.current.loading).toBe(true);
    });

    await act(async () => {
      finish();
      await Promise.resolve();
    });
    expect(result.current.loading).toBe(false);
    expect(result.current.error).toBeNull();
  });

  it("surfaces the error text of an ApiError", async () => {
    const action = () => Promise.reject(new ApiError(409, "session is running"));

    const { result } = renderHook(() => useFormSubmit(action));
    await act(async () => {
      await result.current.submit();
    });

    expect(result.current.error).toBe("session is running");
    expect(result.current.loading).toBe(false);
  });

  it("reports anything else generically and logs it", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const boom = new TypeError("Failed to fetch");
    const action = () => Promise.reject(boom);

    const { result } = renderHook(() => useFormSubmit(action));
    await act(async () => {
      await result.current.submit();
    });

    expect(result.current.error).toBe("Something went wrong");
    expect(logged).toHaveBeenCalledWith(boom);
  });

  it("ignores a second submit while the first is in flight", async () => {
    let finish = () => {};
    const action = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    );

    const { result } = renderHook(() => useFormSubmit(action));
    await act(async () => {
      void result.current.submit();
      void result.current.submit();
      await Promise.resolve();
    });

    expect(action).toHaveBeenCalledTimes(1);

    await act(async () => {
      finish();
      await Promise.resolve();
    });
    expect(result.current.loading).toBe(false);
  });

  it("clears the error on request and on the next submit", async () => {
    let fail = true;
    const action = () =>
      fail ? Promise.reject(new ApiError(400, "username is taken")) : Promise.resolve();

    const { result } = renderHook(() => useFormSubmit(action));
    await act(async () => {
      await result.current.submit();
    });
    expect(result.current.error).toBe("username is taken");

    act(() => {
      result.current.clearError();
    });
    expect(result.current.error).toBeNull();

    fail = true;
    await act(async () => {
      await result.current.submit();
    });
    expect(result.current.error).toBe("username is taken");

    fail = false;
    await act(async () => {
      await result.current.submit();
    });
    expect(result.current.error).toBeNull();
  });
});
