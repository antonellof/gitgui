import Foundation
import CoreGraphics

// usage: click <x> <y> [move|click|rclick|down|up|scroll <dy>]
let args = CommandLine.arguments
guard args.count >= 3, let x = Double(args[1]), let y = Double(args[2]) else {
    print("usage: click x y [move|click|down|up|scroll dy]"); exit(1)
}
let kind = args.count > 3 ? args[3] : "click"
let p = CGPoint(x: x, y: y)
func post(_ type: CGEventType, _ button: CGMouseButton = .left) {
    let e = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: p, mouseButton: button)!
    e.post(tap: .cghidEventTap)
}
switch kind {
case "move": post(.mouseMoved)
case "down": post(.mouseMoved); usleep(20000); post(.leftMouseDown)
case "up": post(.leftMouseUp)
case "drag": post(.leftMouseDragged)
case "rclick": post(.mouseMoved); usleep(20000); post(.rightMouseDown, .right); usleep(30000); post(.rightMouseUp, .right)
case "scroll":
    let dy = Int32(args.count > 4 ? args[4] : "-3") ?? -3
    post(.mouseMoved); usleep(20000)
    let e = CGEvent(scrollWheelEvent2Source: nil, units: .line, wheelCount: 1, wheel1: dy, wheel2: 0, wheel3: 0)!
    e.location = p
    e.post(tap: .cghidEventTap)
default: post(.mouseMoved); usleep(20000); post(.leftMouseDown); usleep(40000); post(.leftMouseUp)
}
usleep(30000)
