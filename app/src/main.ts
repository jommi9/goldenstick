import "./styles.css";
import { createApi } from "./api";
import { start } from "./app";

createApi()
  .then(start)
  .catch((e) => {
    document.getElementById("root")!.textContent = `BoothReady couldn't start: ${e}`;
  });
