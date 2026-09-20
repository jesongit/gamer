import * as m0 from "../api"
import * as m1 from "../gamer-plugin-ids"
import * as m2 from "vue"
import * as m3 from "../store"
import * as m4 from "../components/ui/useConfirmDialog"
import * as m5 from "../components/ui/UiIcon.vue"
import * as m6 from "../components/ui/useOperationStatus"
import * as m7 from "../runs"
import * as m8 from "../components/console/current-api-adapters"
import * as m9 from "../workspace/operation-feedback"
import * as m10 from "../console/geometry"
import * as m11 from "../auth"
import * as m12 from "vue-router"
import * as m13 from "../workspace/context"
import * as m14 from "../components/console/useConsoleStage"
import * as m15 from "../package-store"
import * as m16 from "../keymap-control"
import * as m17 from "../components/task/runner-editors"
import * as m18 from "../workspace/plugin-messages"

export function installPluginModuleSdk() {
  globalThis.__gamerPluginSdkV1 = Object.freeze({
  "api": m0,
  "gamer-plugin-ids": m1,
  "vue": m2,
  "store": m3,
  "components/ui/useConfirmDialog": m4,
  "components/ui/UiIcon.vue": m5,
  "components/ui/useOperationStatus": m6,
  "runs": m7,
  "components/console/current-api-adapters": m8,
  "workspace/operation-feedback": m9,
  "console/geometry": m10,
  "auth": m11,
  "vue-router": m12,
  "workspace/context": m13,
  "components/console/useConsoleStage": m14,
  "package-store": m15,
  "keymap-control": m16,
  "components/task/runner-editors": m17,
  "workspace/plugin-messages": m18
  })
}
