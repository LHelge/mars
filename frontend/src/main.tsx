import { QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router";
import { App } from "./App";
import { AuthBootstrap } from "./AuthBootstrap";
import { queryClient } from "./queryClient";
import "./index.css";

const container = document.getElementById("root");
if (!container) {
  throw new Error("missing #root element");
}

createRoot(container).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      {/* Inside the router: the bootstrap navigates on sign-out and on a
          `password change required` 403. */}
      <BrowserRouter>
        <AuthBootstrap>
          <App />
        </AuthBootstrap>
      </BrowserRouter>
    </QueryClientProvider>
  </StrictMode>,
);
