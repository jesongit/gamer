// Integration tests use the same entry points as the independently built assets.
import { pluginModules } from './workspace/plugin-module-registry'
import * as yaml from '../../plugins/gamer-yaml/ui/entry'
import * as keymap from '../../plugins/gamer-keymap/ui/entry'
import * as video from '../../plugins/gamer-video/ui/entry'
pluginModules.set('gamer-yaml', yaml)
pluginModules.set('gamer-keymap', keymap)
pluginModules.set('gamer-video', video)
