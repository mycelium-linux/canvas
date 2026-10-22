import UIKit
import AVFoundation

enum CanvasBridge {
    static func request(_ object: [String: Any]) throws -> [String: Any] {
        let input = try JSONSerialization.data(withJSONObject: object)
        var output = [UInt8](repeating: 0, count: 65536)
        let length = input.withUnsafeBytes { bytes in
            canvas_tvos_request(bytes.bindMemory(to: UInt8.self).baseAddress!, input.count,
                                &output, output.count)
        }
        guard length > 0 else { throw NSError(domain: "Canvas ABI", code: Int(length)) }
        let reply = try JSONSerialization.jsonObject(with: Data(output.prefix(Int(length)))) as! [String: Any]
        guard reply["ok"] as? Bool == true else {
            throw NSError(domain: reply["error"] as? String ?? "Canvas", code: 1)
        }
        return reply["result"] as! [String: Any]
    }
    static func color(_ value: Any?) -> UIColor {
        guard let values = value as? [NSNumber], values.count == 4 else { return .clear }
        return UIColor(red: CGFloat(values[0].doubleValue / 255),
                       green: CGFloat(values[1].doubleValue / 255),
                       blue: CGFloat(values[2].doubleValue / 255),
                       alpha: CGFloat(values[3].doubleValue / 255))
    }
}

// Raster adapter only: geometry/colors come from the canonical GFX scene.
// Metal/wGPU surface hosting is a separate qualification slice, not claimed here.
final class CanvasSceneView: UIView {
    var failure: ((String) -> Void)?
    override func draw(_ rect: CGRect) {
        guard bounds.width >= 180, bounds.height >= 180 else { return }
        do {
            let now = Calendar.current.dateComponents([.hour, .minute, .second, .nanosecond], from: Date())
            let frame = try CanvasBridge.request(["action":"frame", "width":bounds.width,
                "height":bounds.height,"hour":now.hour!,"minute":now.minute!,
                "second":Double(now.second!) + Double(now.nanosecond!) / 1e9])
            guard let context = UIGraphicsGetCurrentContext() else { return }
            context.setFillColor(CanvasBridge.color(frame["background"]).cgColor)
            context.fill(bounds)
            for item in frame["primitives"] as! [[String: Any]] {
                let r = item["rect"] as! [NSNumber]
                let box = CGRect(x:r[0].doubleValue,y:r[1].doubleValue,width:r[2].doubleValue,height:r[3].doubleValue)
                let kind = item["kind"] as! Int
                let path: UIBezierPath
                if kind == 5 {
                    let half = (item["value"] as! NSNumber).doubleValue
                    let angle = (item["rotation"] as! NSNumber).doubleValue
                    path = UIBezierPath()
                    path.move(to: CGPoint(x:box.midX - cos(angle) * half,y:box.midY - sin(angle) * half))
                    path.addLine(to: CGPoint(x:box.midX + cos(angle) * half,y:box.midY + sin(angle) * half))
                    path.lineWidth = CGFloat((item["radius"] as! NSNumber).doubleValue * 2)
                    path.lineCapStyle = .round
                    CanvasBridge.color(item["fill"]).setStroke()
                    path.stroke()
                    continue
                }
                path = kind == 1 ? UIBezierPath(ovalIn:box) :
                    UIBezierPath(roundedRect:box,cornerRadius:CGFloat((item["radius"] as! NSNumber).doubleValue))
                CanvasBridge.color(item["fill"]).setFill()
                path.fill()
                path.lineWidth = CGFloat((item["stroke"] as! NSNumber).doubleValue)
                CanvasBridge.color(item["outline"]).setStroke()
                if path.lineWidth > 0 { path.stroke() }
            }
        } catch { failure?(error.localizedDescription) }
    }
}

final class PlayerSurface: UIView {
    override class var layerClass: AnyClass { AVPlayerLayer.self }
    var playerLayer: AVPlayerLayer { layer as! AVPlayerLayer }
}

final class CanvasController: UIViewController {
    private let scene = CanvasSceneView()
    private let message = UILabel()
    private let video = PlayerSurface()
    private var timer: Timer?
    private var player: AVPlayer?
    private var playerObservation: NSKeyValueObservation?
    override func loadView() {
        view = scene
        scene.failure = { [weak self] in self?.message.text = $0 }
        let bar = UIStackView()
        bar.axis = .horizontal
        bar.spacing = 24
        for (name, id) in [("Home","home"),("Studio","studio"),("Video","video")] {
            let button = UIButton(type:.system)
            button.setTitle(name,for:.normal)
            button.addAction(UIAction { [weak self] _ in self?.activate(id) },for:.primaryActionTriggered)
            bar.addArrangedSubview(button)
        }
        bar.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(bar)
        message.text = "fungOS · Canvas Apple TV / simulator build spike"
        message.font = .systemFont(ofSize:20,weight:.medium)
        message.textAlignment = .center
        message.textColor = .white
        message.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(message)
        video.isHidden = true
        video.playerLayer.videoGravity = .resizeAspect
        video.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(video)
        NSLayoutConstraint.activate([
            bar.leadingAnchor.constraint(equalTo:view.safeAreaLayoutGuide.leadingAnchor,constant:20),
            bar.topAnchor.constraint(equalTo:view.topAnchor,constant:16),
            message.centerXAnchor.constraint(equalTo:view.centerXAnchor),
            message.bottomAnchor.constraint(equalTo:view.safeAreaLayoutGuide.bottomAnchor,constant:-12),
            video.leadingAnchor.constraint(equalTo:view.leadingAnchor,constant:100),
            video.trailingAnchor.constraint(equalTo:view.trailingAnchor,constant:-100),
            video.topAnchor.constraint(equalTo:view.topAnchor,constant:110),
            video.bottomAnchor.constraint(equalTo:message.topAnchor,constant:-32)
        ])
    }
    func setActive(_ active: Bool) {
        timer?.invalidate()
        timer = nil
        if active {
            timer = Timer.scheduledTimer(withTimeInterval:1.0 / 30.0,repeats:true) { [weak self] _ in self?.scene.setNeedsDisplay() }
        } else { player?.pause() }
    }
    private func activate(_ id: String) {
        if id == "video" {
            // Public Apple HLS test asset; no Unibus stream integration yet.
            let url = URL(string:"https://devstreaming-cdn.apple.com/videos/streaming/examples/bipbop_4x3/bipbop_4x3_variant.m3u8")!
            player = AVPlayer(url:url)
            video.playerLayer.player = player
            video.isHidden = false
            message.text = "Native AVPlayer · public HLS sample · not yet Unibus"
            playerObservation = player?.currentItem?.observe(\.status,options:[.new]) { [weak self] item, _ in
                if item.status == .failed {
                    DispatchQueue.main.async { self?.message.text = item.error?.localizedDescription ?? "Video failed" }
                }
            }
            player?.play()
            return
        }
        player?.pause()
        video.isHidden = true
        do {
            let snapshot = try CanvasBridge.request(["action":"snapshot"])
            let reply = try CanvasBridge.request(["action":"select","project":id,
                "expected_revision":snapshot["revision"]!])
            message.text = "\(id.capitalized) · shared Canvas session · revision \(reply["revision"]!)"
            scene.setNeedsDisplay()
        } catch { message.text = error.localizedDescription }
    }
}

@main
final class CanvasApplication: UIResponder, UIApplicationDelegate {
    var window: UIWindow?
    private let controller = CanvasController()
    func application(_ application: UIApplication,didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey:Any]? = nil) -> Bool {
        window = UIWindow(frame:UIScreen.main.bounds)
        window?.rootViewController = controller
        window?.makeKeyAndVisible()
        return true
    }
    func applicationDidBecomeActive(_ application: UIApplication) { controller.setActive(true) }
    func applicationWillResignActive(_ application: UIApplication) { controller.setActive(false) }
}
