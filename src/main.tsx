import React from "react";
import ReactDOM from "react-dom/client";
import "katex/dist/katex.min.css"; // the manual's math — fonts bundled, 0 network refs (offline)
import App from "./App";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
