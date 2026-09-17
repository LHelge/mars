import { Route, Routes } from "react-router";
import { HealthPlaceholderPage } from "./pages/HealthPlaceholderPage";

export function App() {
  return (
    <Routes>
      <Route path="/" element={<HealthPlaceholderPage />} />
    </Routes>
  );
}
