// Foreign TypeScript fixture: express-style router with higher-order
// middleware registration and cross-module handler types.
import type { Request, Response } from "./http.js";
import { logRequest } from "./logging.js";

type Handler = (req: Request, res: Response) => void;

interface Router {
  use(handler: Handler): void;
  handle(req: Request, res: Response): void;
}

function healthHandler(req: Request, res: Response): void {
  logRequest(req);
  res.end("ok");
}

class SimpleRouter implements Router {
  private handlers: Handler[] = [];

  use(handler: Handler): void {
    this.handlers.push(handler);
  }

  handle(req: Request, res: Response): void {
    for (const handler of this.handlers) {
      handler(req, res);
    }
  }
}

function mount(router: Router): void {
  router.use(healthHandler);
}

function neverMounted(req: Request, res: Response): void {
  res.end("nope");
}
