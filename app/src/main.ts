// M PLUS 1p, bundled so the app needs no network. Latin Extended covers
// accented track names; other scripts fall back to the system font.
import "@fontsource/m-plus-1p/latin-300.css";
import "@fontsource/m-plus-1p/latin-400.css";
import "@fontsource/m-plus-1p/latin-500.css";
import "@fontsource/m-plus-1p/latin-700.css";
import "@fontsource/m-plus-1p/latin-ext-300.css";
import "@fontsource/m-plus-1p/latin-ext-400.css";
import "@fontsource/m-plus-1p/latin-ext-500.css";
import "@fontsource/m-plus-1p/latin-ext-700.css";
import "./styles.css";
import { createApi } from "./api";
import { start } from "./app";

createApi()
  .then(start)
  .catch((e) => {
    document.getElementById("root")!.textContent = `BoothReady couldn't start: ${e}`;
  });
