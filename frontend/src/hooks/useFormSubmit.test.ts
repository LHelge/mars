import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../services/apiClient";
import { statusMessage } from "../services/errorMessage";
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

  it("names a network failure and does not log it", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const action = () => Promise.reject(new TypeError("Failed to fetch"));

    const { result } = renderHook(() => useFormSubmit(action));
    await act(async () => {
      await result.current.submit();
    });

    expect(result.current.error).toBe("Orchestrator unreachable");
    expect(logged).not.toHaveBeenCalled();
  });

  it("reports a bug here generically and logs it once", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const boom = new Error("boom");
    const action = () => Promise.reject(boom);

    const { result } = renderHook(() => useFormSubmit(action));
    await act(async () => {
      await result.current.submit();
    });

    expect(result.current.error).toBe("Something went wrong");
    expect(logged).toHaveBeenCalledTimes(1);
    expect(logged).toHaveBeenCalledWith(boom);
  });

  it("shows something for a proxy's HTML 502, whose body is not the envelope", async () => {
    // What `apiClient` builds when nginx answers with HTML and `statusText`
    // is empty, as it always is under HTTP/2 and HTTP/3.
    const action = () => Promise.reject(new ApiError(502, statusMessage(502)));

    const { result } = renderHook(() => useFormSubmit(action));
    await act(async () => {
      await result.current.submit();
    });

    expect(result.current.error).toBe("Orchestrator unreachable");
  });

  it("lets a form word a failure itself, without forging an ApiError", async () => {
    const action = () => Promise.reject(new ApiError(401, "unauthenticated"));
    const mapError = (caught: unknown) =>
      caught instanceof ApiError && caught.status === 401
        ? "Invalid username or password"
        : "Something went wrong";

    const { result } = renderHook(() => useFormSubmit(action, { mapError }));
    await act(async () => {
      await result.current.submit();
    });

    expect(result.current.error).toBe("Invalid username or password");
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

  it("says a submission succeeded until the next one, or a reset", async () => {
    let fail = false;
    const action = () =>
      fail ? Promise.reject(new ApiError(409, "already there")) : Promise.resolve();

    const { result } = renderHook(() => useFormSubmit(action));
    expect(result.current.succeeded).toBe(false);

    await act(async () => {
      await result.current.submit();
    });
    expect(result.current.succeeded).toBe(true);

    act(() => {
      result.current.reset();
    });
    expect(result.current.succeeded).toBe(false);

    await act(async () => {
      await result.current.submit();
    });
    expect(result.current.succeeded).toBe(true);

    // A failure is never also a success, whatever the last attempt said.
    fail = true;
    await act(async () => {
      await result.current.submit();
    });
    expect(result.current.succeeded).toBe(false);
    expect(result.current.error).toBe("already there");
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
      result.current.reset();
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
