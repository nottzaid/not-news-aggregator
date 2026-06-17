import 'dart:async';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import 'package:record/record.dart';
import 'package:url_launcher/url_launcher.dart';

import 'canvas/canvas_layout.dart';
import 'data/fixture_events.dart';
import 'data/graph_repository.dart';
import 'data/research_session_client.dart';
import 'models/research_event.dart';

// ── Nocturne palette ───────────────────────────────────────────────────────
// A dark "intelligence observatory" canvas: deep ink, warm signal amber,
// cool data cyan, and a violet accent. Event colors ride on top as glows.
const _ink0 = Color(0xff06090f);
const _ink1 = Color(0xff0b0f18);
const _ink2 = Color(0xff111726);
const _panel = Color(0xff0f1420);
const _panelRaised = Color(0xff151b2b);
const _hairline = Color(0xff2a3346);
const _hairlineDim = Color(0x332a3346);
const _inkText = Color(0xffece6d6);
const _inkTextDim = Color(0xff97a0b4);
const _inkTextFaint = Color(0xff5a6478);
const _signal = Color(0xffe8a44c);
const _signalHot = Color(0xffff6b4a);
const _signalHotDeep = Color(0xffc23a24);
const _data = Color(0xff4cc9d6);
const _plum = Color(0xff7c5cff);
const _bridge = Color(0xff39445a);
const _bridgeHi = Color(0xff8b97b5);
const _gridLine = Color(0x0bffffff);
const _gridMajor = Color(0x12ffffff);

const _display = 'Manrope';
const _mono = 'JetBrainsMono';

void main() {
  runApp(const AiNewsCanvasApp());
}

class AiNewsCanvasApp extends StatelessWidget {
  const AiNewsCanvasApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      title: 'AI News Canvas',
      theme: ThemeData(
        useMaterial3: true,
        fontFamily: _display,
        brightness: Brightness.dark,
        scaffoldBackgroundColor: _ink0,
        canvasColor: _ink1,
        colorScheme: ColorScheme.fromSeed(
          seedColor: _signal,
          brightness: Brightness.dark,
        ),
        iconTheme: const IconThemeData(color: _inkTextDim, size: 18),
        tooltipTheme: const TooltipThemeData(
          textStyle: TextStyle(
            fontFamily: _mono,
            color: _inkText,
            fontSize: 11,
            fontWeight: FontWeight.w600,
            letterSpacing: 0.4,
          ),
          decoration: BoxDecoration(
            color: _panelRaised,
            border: Border.fromBorderSide(
              BorderSide(color: _hairline, width: 1),
            ),
            borderRadius: BorderRadius.all(Radius.circular(4)),
          ),
        ),
      ),
      home: const CanvasPrototypeScreen(),
    );
  }
}

class CanvasPrototypeScreen extends StatefulWidget {
  const CanvasPrototypeScreen({super.key});

  @override
  State<CanvasPrototypeScreen> createState() => _CanvasPrototypeScreenState();
}

class _CanvasPrototypeScreenState extends State<CanvasPrototypeScreen>
    with TickerProviderStateMixin {
  static const _researchPrompt =
      'What is there to know about the Anthropic-SpaceX deal?';
  static const _debugCamera =
      bool.fromEnvironment('AI_NEWS_DEBUG_CAMERA', defaultValue: false);

  late final AnimationController _motion;
  late final AnimationController _cameraMotion;
  late final AnimationController _bridgeFlow;
  late final AnimationController _artifactHover;
  late final CanvasGraphRepository _graphRepository;
  late final ResearchSessionClient _researchSessionClient;
  late final AudioRecorder _audioRecorder;
  late final _CanvasViewportController _canvasViewport;
  late Map<String, Offset> _basePositions;
  List<ResearchEvent> _events = fixtureEvents;
  List<EventBridge> _bridges = fixtureBridges;
  List<String> _progressMessages = const [];
  bool _hermesPanelOpen = false;
  StreamSubscription<CanvasGraphState>? _graphSubscription;
  Set<String> _sessionGeneratedEventIds = {};
  Set<String> _sessionFocusEventIds = {};
  Set<String>? _pendingFocusEventIds;
  bool _autoFollowGeneratedCluster = true;
  String? _activeId;
  String? _hoveredArtifactUrl;
  String? _sessionMessage;
  bool _sessionRunning = false;
  bool _recording = false;
  bool _transcribing = false;
  bool _clearingCanvas = false;
  String? _recordingPath;
  String? _motionFromActiveId;
  String? _motionToActiveId;
  Map<String, EventLayout>? _motionFromLayouts;
  Map<String, EventLayout>? _motionToLayouts;
  Map<String, EventLayout>? _settledLayouts;
  List<ResearchEvent>? _settledLayoutEvents;
  Map<String, Offset>? _settledLayoutBasePositions;
  String? _settledLayoutActiveId;
  Map<String, double> _settledExpansionProgresses = const {};
  String? _settledExpansionActiveId;
  Timer? _collapseTimer;
  Offset? _panStart;
  Offset? _cameraStart;
  Offset? _cameraMotionFrom;
  Offset? _cameraMotionTo;
  double? _panZoomStartZoom;
  Size? _viewportSize;
  bool _isPanning = false;

  @override
  void initState() {
    super.initState();
    _graphRepository = CanvasGraphRepository();
    _researchSessionClient = const ResearchSessionClient();
    _audioRecorder = AudioRecorder();
    _canvasViewport = _CanvasViewportController();
    _basePositions = generateBasePositions(_events, bridges: _bridges);
    _motion = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 220),
    )..addStatusListener((status) {
        if (status == AnimationStatus.completed) {
          _motionFromLayouts = null;
          _motionToLayouts = null;
          _motionFromActiveId = null;
          _motionToActiveId = null;
          if (_activeId == null) {
            _bridgeFlow.stop();
          }
          final pendingFocus = _pendingFocusEventIds;
          if (pendingFocus != null && pendingFocus.isNotEmpty) {
            _pendingFocusEventIds = null;
            WidgetsBinding.instance.addPostFrameCallback((_) {
              if (mounted) {
                _focusEvents(pendingFocus, _basePositions);
              }
            });
          }
        }
      });
    _cameraMotion = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 560),
    )
      ..addListener(() {
        final from = _cameraMotionFrom;
        final to = _cameraMotionTo;
        if (from == null || to == null) {
          return;
        }
        final progress = Curves.easeInOutCubic.transform(_cameraMotion.value);
        _canvasViewport.setCamera(
          _clampedCamera(Offset.lerp(from, to, progress)!),
        );
      })
      ..addStatusListener((status) {
        if (status == AnimationStatus.completed ||
            status == AnimationStatus.dismissed) {
          _cameraMotionFrom = null;
          _cameraMotionTo = null;
        }
      });
    _bridgeFlow = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 7000),
    );
    _artifactHover = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 170),
    )..addStatusListener((status) {
        if (status == AnimationStatus.dismissed &&
            _hoveredArtifactUrl != null) {
          setState(() => _hoveredArtifactUrl = null);
        }
      });
    _connectGraphStream();
  }

  @override
  void dispose() {
    _graphSubscription?.cancel();
    _audioRecorder.dispose();
    _collapseTimer?.cancel();
    _artifactHover.dispose();
    _bridgeFlow.dispose();
    _cameraMotion.dispose();
    _canvasViewport.dispose();
    _motion.dispose();
    super.dispose();
  }

  Offset get _camera => _canvasViewport.camera;

  double get _zoom => _canvasViewport.zoom;

  @override
  Widget build(BuildContext context) {
    final targetLayouts = _settledDisplayLayouts();
    final activeLayout =
        _activeId == null ? null : _interactiveLayouts()[_activeId!];

    return Scaffold(
      body: LayoutBuilder(
        builder: (context, constraints) {
          final size = Size(constraints.maxWidth, constraints.maxHeight);
          _viewportSize = size;
          return Stack(
            fit: StackFit.expand,
            children: [
              const RepaintBoundary(
                child: CustomPaint(painter: CanvasBackgroundPainter()),
              ),
              Listener(
                onPointerSignal: (event) {
                  if (event is PointerScrollEvent) {
                    _handleScrollZoom(
                        event.localPosition, event.scrollDelta, size);
                  }
                },
                onPointerPanZoomStart: (event) =>
                    _handlePanZoomStart(event.localPosition),
                onPointerPanZoomUpdate: (event) =>
                    _handlePanZoomUpdate(event, size),
                onPointerPanZoomEnd: (_) => _handlePanZoomEnd(),
                onPointerHover: (event) =>
                    _handleHover(event.localPosition, size),
                onPointerDown: (event) {
                  _cancelCollapse();
                  _panStart = event.localPosition;
                  _cameraStart = _camera;
                  _isPanning = false;
                },
                onPointerMove: (event) {
                  if (_panStart == null || _cameraStart == null) {
                    return;
                  }
                  final delta = event.localPosition - _panStart!;
                  if (delta.distance > 4) {
                    _isPanning = true;
                    _autoFollowGeneratedCluster = false;
                    _sessionFocusEventIds = {};
                    _pendingFocusEventIds = null;
                  }
                  _setCamera(_cameraStart! - _screenDeltaToWorld(delta, size));
                },
                onPointerUp: (event) {
                  final wasPanning = _isPanning;
                  _panStart = null;
                  _cameraStart = null;
                  _isPanning = false;
                  if (!wasPanning) {
                    _handleTap(event.localPosition, size);
                  }
                },
                onPointerCancel: (_) {
                  _panStart = null;
                  _cameraStart = null;
                  _isPanning = false;
                },
                child: AnimatedBuilder(
                  animation: _motion,
                  builder: (context, _) {
                    final animatedLayouts =
                        _currentLayouts(fallback: targetLayouts);
                    final expansionProgresses = _expansionProgresses();
                    return Stack(
                      fit: StackFit.expand,
                      children: [
                        RepaintBoundary(
                          child: CustomPaint(
                            isComplex: true,
                            painter: _EventCanvasPainter(
                              repaint: Listenable.merge(
                                [
                                  _bridgeFlow,
                                  _artifactHover,
                                  _canvasViewport,
                                ],
                              ),
                              events: _events,
                              bridges: _bridges,
                              layouts: animatedLayouts,
                              activeId: _activeId,
                              bridgeActiveId: _bridgeActiveId,
                              hoveredArtifactUrl: _hoveredArtifactUrl,
                              artifactHover: _artifactHover,
                              expansionProgresses: expansionProgresses,
                              viewport: _canvasViewport,
                              bridgeFlow: _bridgeFlow,
                            ),
                          ),
                        ),
                      ],
                    );
                  },
                ),
              ),
              if (activeLayout != null)
                _MetadataSheet(
                  layout: activeLayout,
                  viewportSize: size,
                ),
              if (_sessionMessage != null)
                _SessionStatus(
                  message: _sessionMessage!,
                  running: _sessionRunning,
                ),
              if (_progressMessages.isNotEmpty || _sessionRunning)
                _HermesActivityDrawer(
                  messages: _progressMessages,
                  running: _sessionRunning,
                  open: _hermesPanelOpen,
                  onToggle: () =>
                      setState(() => _hermesPanelOpen = !_hermesPanelOpen),
                ),
              _RecordButton(
                running: _sessionRunning || _recording || _transcribing,
                recording: _recording,
                transcribing: _transcribing,
                onPressed: _toggleRecording,
                onCancel: _cancelRecording,
              ),
              _ZoomControls(
                zoom: _canvasViewport.zoomListenable,
                onZoomIn: () => _zoomBy(1.18, size.center(Offset.zero)),
                onZoomOut: () => _zoomBy(1 / 1.18, size.center(Offset.zero)),
                onReset: () => _resetZoom(size),
                clearing: _clearingCanvas,
                clearEnabled: !_sessionRunning &&
                    !_recording &&
                    !_transcribing &&
                    !_clearingCanvas,
                onClear: _clearCanvas,
              ),
            ],
          );
        },
      ),
    );
  }

  void _handleHover(
    Offset screenPoint,
    Size size,
  ) {
    final layouts = _interactiveLayouts();
    final worldPoint = _screenToWorld(screenPoint, size);
    final active = _activeId == null ? null : layouts[_activeId!];

    if (active != null && active.event.canExpand) {
      if (_isProtectedActivePath(worldPoint, active)) {
        final hoveredArtifact = _hitArtifact(worldPoint, active);
        _setHoveredArtifact(hoveredArtifact?.artifact.url);
        _cancelCollapse();
      } else {
        _setHoveredArtifact(null);
        final event = _hitEvent(worldPoint, layouts);
        if (event != null) {
          _setActive(event.event.id);
        } else {
          _scheduleCollapse();
        }
      }
      return;
    }

    final event = _hitEvent(worldPoint, layouts);
    if (event != null) {
      _setHoveredArtifact(null);
      _setActive(event.event.id);
    } else {
      _setHoveredArtifact(null);
      _scheduleCollapse();
    }
  }

  Future<void> _handleTap(
    Offset screenPoint,
    Size size,
  ) async {
    final layouts = _interactiveLayouts();
    final worldPoint = _screenToWorld(screenPoint, size);
    final active = _activeId == null ? null : layouts[_activeId!];
    if (active != null) {
      final artifact = _hitArtifact(worldPoint, active);
      if (artifact != null) {
        await _openUrl(artifact.artifact.url);
        return;
      }
    }

    final hit = _hitEvent(worldPoint, layouts);
    if (hit == null) {
      _clearActive();
      return;
    }

    if (!hit.event.canExpand) {
      await _openEventUrl(hit.event);
      return;
    }

    if (_activeId == hit.event.id) {
      _clearActive();
    } else {
      _setActive(hit.event.id);
    }
  }

  void _setActive(String id) {
    _cancelCollapse();
    if (_activeId == id) {
      return;
    }
    _animateActiveChange(id);
  }

  void _clearActive() {
    _cancelCollapse();
    if (_activeId == null) {
      return;
    }
    _setHoveredArtifact(null);
    _animateActiveChange(null);
  }

  void _scheduleCollapse() {
    if (_collapseTimer != null || _activeId == null) {
      return;
    }
    _collapseTimer = Timer(const Duration(milliseconds: 180), () {
      _collapseTimer = null;
      if (mounted) {
        _clearActive();
      }
    });
  }

  void _cancelCollapse() {
    _collapseTimer?.cancel();
    _collapseTimer = null;
  }

  void _setHoveredArtifact(String? url) {
    if (_hoveredArtifactUrl == url) {
      if (url != null && _artifactHover.value < 1) {
        _artifactHover.forward();
      }
      return;
    }
    if (url == null) {
      if (_hoveredArtifactUrl != null) {
        setState(() => _hoveredArtifactUrl = null);
      }
      _artifactHover.reverse();
      return;
    }
    setState(() => _hoveredArtifactUrl = url);
    _artifactHover.forward(from: 0);
  }

  EventLayout? _hitEvent(Offset worldPoint, Map<String, EventLayout> layouts) {
    for (final layout in layouts.values) {
      if ((worldPoint - layout.display).distance <= 54) {
        return layout;
      }
    }
    return null;
  }

  ArtifactLayout? _hitArtifact(Offset worldPoint, EventLayout active) {
    if (!active.event.canExpand || _activeId != active.event.id) {
      return null;
    }
    for (final artifact in active.artifacts) {
      final center = active.display + artifact.offset;
      if ((worldPoint - center).distance <= artifact.radius) {
        return artifact;
      }
    }
    return null;
  }

  bool _isProtectedActivePath(Offset worldPoint, EventLayout active) {
    if ((worldPoint - active.display).distance <= 46) {
      return true;
    }
    for (final artifact in active.artifacts) {
      final start = active.display;
      final end = active.display + artifact.offset;
      if ((worldPoint - end).distance <= artifact.radius + 4) {
        return true;
      }
      if (_distanceToSegment(worldPoint, start, end) <= 14) {
        return true;
      }
    }
    return false;
  }

  void _setCamera(Offset camera) {
    _cameraMotion.stop();
    _canvasViewport.setCamera(_clampedCamera(camera));
  }

  void _handleScrollZoom(Offset screenPoint, Offset scrollDelta, Size size) {
    if (scrollDelta.dy == 0) {
      return;
    }
    final factor = math.exp(-scrollDelta.dy * 0.0016);
    _setZoom(_zoom * factor, anchor: screenPoint, size: size);
  }

  void _handlePanZoomStart(Offset screenPoint) {
    _cancelCollapse();
    _cameraMotion.stop();
    _panZoomStartZoom = _zoom;
    _panStart = null;
    _cameraStart = null;
    _isPanning = true;
    _autoFollowGeneratedCluster = false;
    _sessionFocusEventIds = {};
    _pendingFocusEventIds = null;
  }

  void _handlePanZoomUpdate(PointerPanZoomUpdateEvent event, Size size) {
    final startZoom = _panZoomStartZoom ?? _zoom;
    final targetZoom = startZoom * event.scale;
    if ((targetZoom - _zoom).abs() >= 0.002) {
      _setZoom(targetZoom, anchor: event.localPosition, size: size);
    }
    if (event.panDelta.distance > 0) {
      _setCamera(_camera - _screenDeltaToWorld(event.panDelta, size));
    }
  }

  void _handlePanZoomEnd() {
    _panZoomStartZoom = null;
    _isPanning = false;
  }

  void _zoomBy(double factor, Offset anchor) {
    final size = _viewportSize;
    if (size == null) {
      return;
    }
    _setZoom(_zoom * factor, anchor: anchor, size: size);
  }

  void _resetZoom(Size size) {
    _setZoom(1, anchor: size.center(Offset.zero), size: size);
  }

  void _setZoom(
    double zoom, {
    required Offset anchor,
    required Size size,
  }) {
    final nextZoom = zoom.clamp(0.35, 2.8).toDouble();
    if ((nextZoom - _zoom).abs() < 0.002) {
      return;
    }

    _cameraMotion.stop();
    final worldAnchor = _screenToWorld(anchor, size);
    final nextTransform = _CanvasTransform(
      size: size,
      camera: _camera,
      zoom: nextZoom,
    );
    final nextCamera = worldAnchor -
        Offset(
          (anchor.dx - nextTransform.origin.dx) / nextTransform.scale,
          (anchor.dy - nextTransform.origin.dy) / nextTransform.scale,
        );

    _canvasViewport.setView(
      camera: _clampedCamera(nextCamera),
      zoom: nextZoom,
    );
  }

  Offset _clampedCamera(Offset camera) {
    return camera;
  }

  void _animateCameraTo(Offset camera) {
    final target = _clampedCamera(camera);
    if ((target - _camera).distance < 2) {
      return;
    }
    _cameraMotion.stop();
    _cameraMotionFrom = _camera;
    _cameraMotionTo = target;
    _cameraMotion.forward(from: 0);
  }

  Offset _screenToWorld(Offset screenPoint, Size size) {
    final transform =
        _CanvasTransform(size: size, camera: _camera, zoom: _zoom);
    return transform.screenToWorld(screenPoint);
  }

  Offset _screenDeltaToWorld(Offset delta, Size size) {
    final transform =
        _CanvasTransform(size: size, camera: _camera, zoom: _zoom);
    return Offset(delta.dx / transform.scale, delta.dy / transform.scale);
  }

  double _distanceToSegment(Offset point, Offset start, Offset end) {
    final line = end - start;
    final lengthSquared = line.dx * line.dx + line.dy * line.dy;
    if (lengthSquared == 0) {
      return (point - start).distance;
    }
    final t =
        (((point.dx - start.dx) * line.dx + (point.dy - start.dy) * line.dy) /
                lengthSquared)
            .clamp(0.0, 1.0);
    final projection = start + line * t;
    return (point - projection).distance;
  }

  Future<void> _openUrl(String url) async {
    final uri = Uri.parse(url);
    await launchUrl(uri, mode: LaunchMode.externalApplication);
  }

  Future<void> _openEventUrl(ResearchEvent event) async {
    final url = event.directUrl;
    if (url == null) {
      return;
    }
    await _openUrl(url);
  }

  void _animateActiveChange(String? nextId) {
    final from = _currentLayouts(
      fallback: _settledDisplayLayouts(),
    );
    final previousId = _activeId;
    final to = displayLayout(
      events: _events,
      basePositions: _basePositions,
      activeId: nextId,
    );
    _cacheSettledDisplayLayouts(
      to,
      events: _events,
      basePositions: _basePositions,
      activeId: nextId,
    );

    _motion.stop();
    _motionFromLayouts = from;
    _motionToLayouts = to;
    _motionFromActiveId = previousId;
    _motionToActiveId = nextId;
    _motion.value = 0;
    _setBridgeFlowActive(nextId ?? previousId);
    setState(() => _activeId = nextId);
    _motion.forward(from: 0);
  }

  Map<String, EventLayout> _currentLayouts({
    required Map<String, EventLayout> fallback,
  }) {
    final from = _motionFromLayouts;
    final to = _motionToLayouts;
    if (from == null || to == null || !_motion.isAnimating) {
      return fallback;
    }

    final progress = Curves.easeOutCubic.transform(_motion.value);
    return {
      for (final event in _events)
        event.id: from[event.id] == null || to[event.id] == null
            ? to[event.id]!
            : to[event.id]!.copyWith(
                display: Offset.lerp(
                    from[event.id]!.display, to[event.id]!.display, progress)!,
              ),
    };
  }

  Map<String, EventLayout> _interactiveLayouts() {
    return _currentLayouts(
      fallback: _settledDisplayLayouts(),
    );
  }

  Map<String, EventLayout> _settledDisplayLayouts() {
    final cached = _settledLayouts;
    if (cached != null &&
        identical(_settledLayoutEvents, _events) &&
        identical(_settledLayoutBasePositions, _basePositions) &&
        _settledLayoutActiveId == _activeId) {
      return cached;
    }

    final layouts = displayLayout(
      events: _events,
      basePositions: _basePositions,
      activeId: _activeId,
    );
    _cacheSettledDisplayLayouts(
      layouts,
      events: _events,
      basePositions: _basePositions,
      activeId: _activeId,
    );
    return layouts;
  }

  void _cacheSettledDisplayLayouts(
    Map<String, EventLayout> layouts, {
    required List<ResearchEvent> events,
    required Map<String, Offset> basePositions,
    required String? activeId,
  }) {
    _settledLayouts = layouts;
    _settledLayoutEvents = events;
    _settledLayoutBasePositions = basePositions;
    _settledLayoutActiveId = activeId;
  }

  Map<String, double> _expansionProgresses() {
    if (_motionFromActiveId == null && _motionToActiveId == null) {
      if (_activeId == null) {
        _settledExpansionActiveId = null;
        _settledExpansionProgresses = const {};
        return _settledExpansionProgresses;
      }
      if (_settledExpansionActiveId != _activeId) {
        _settledExpansionActiveId = _activeId;
        _settledExpansionProgresses = {_activeId!: 1};
      }
      return _settledExpansionProgresses;
    }

    final progress = Curves.easeOutCubic.transform(_motion.value);
    return {
      if (_motionFromActiveId != null) _motionFromActiveId!: 1 - progress,
      if (_motionToActiveId != null) _motionToActiveId!: progress,
    };
  }

  String? get _bridgeActiveId {
    return _activeId ?? _motionFromActiveId;
  }

  void _setBridgeFlowActive(String? id) {
    if (id == null) {
      _bridgeFlow.stop();
      return;
    }
    if (!_bridgeFlow.isAnimating) {
      _bridgeFlow.repeat();
    }
  }

  void _connectGraphStream({Uri? uri, bool startsSession = false}) {
    _graphSubscription?.cancel();
    if (startsSession) {
      _sessionGeneratedEventIds = {};
      _sessionFocusEventIds = {};
      _pendingFocusEventIds = null;
      _autoFollowGeneratedCluster = true;
    }
    _graphSubscription = _graphRepository
        .watch(
      uri: uri,
      startsSession: startsSession,
      initialEvents: startsSession ? _events : null,
      initialBridges: startsSession ? _bridges : null,
    )
        .listen(
      _applyGraphState,
      onDone: () {
        if (mounted) {
          setState(() {
            _graphSubscription = null;
            _sessionRunning = false;
          });
        }
      },
    );
  }

  void _startResearchSession([String prompt = _researchPrompt]) {
    setState(() => _hermesPanelOpen = true);
    final uri = Uri.parse(defaultGraphStreamUri).replace(
      path: '/research/stream',
      queryParameters: {'prompt': prompt},
    );
    _connectGraphStream(uri: uri, startsSession: true);
  }

  Future<void> _startRecording() async {
    if (_sessionRunning || _recording || _transcribing) {
      return;
    }
    try {
      final allowed = await _audioRecorder.hasPermission();
      if (!allowed) {
        setState(
            () => _sessionMessage = 'Microphone permission was not granted.');
        return;
      }
      final directory = await getTemporaryDirectory();
      final path =
          '${directory.path}/ai-news-recording-${DateTime.now().microsecondsSinceEpoch}.wav';
      await _audioRecorder.start(
        const RecordConfig(
          encoder: AudioEncoder.wav,
          sampleRate: 16000,
          numChannels: 1,
          echoCancel: true,
          noiseSuppress: true,
        ),
        path: path,
      );
      setState(() {
        _recording = true;
        _recordingPath = path;
        _sessionMessage = 'Listening... tap again to research.';
      });
    } catch (error) {
      setState(() {
        _recording = false;
        _recordingPath = null;
        _sessionMessage = 'Could not start recording: $error';
      });
    }
  }

  Future<void> _finishRecording() async {
    if (!_recording) {
      return;
    }
    setState(() {
      _recording = false;
      _transcribing = true;
      _sessionMessage = 'Transcribing with Groq Whisper v3 Turbo...';
    });
    try {
      final path = await _audioRecorder.stop() ?? _recordingPath;
      if (path == null) {
        throw StateError('Recorder did not return an audio path.');
      }
      final prompt = await _researchSessionClient.transcribeRecording(path);
      if (!mounted) {
        return;
      }
      setState(() {
        _transcribing = false;
        _sessionMessage = 'Transcript: $prompt';
      });
      _startResearchSession(prompt);
    } catch (error) {
      if (!mounted) {
        return;
      }
      setState(() {
        _transcribing = false;
        _sessionMessage = 'Recording failed: ${_formatRecordingError(error)}';
      });
    } finally {
      _recordingPath = null;
    }
  }

  String _formatRecordingError(Object error) {
    return error.toString().replaceFirst('Bad state: ', '');
  }

  Future<void> _toggleRecording() {
    return _recording ? _finishRecording() : _startRecording();
  }

  Future<void> _cancelRecording() async {
    if (!_recording) {
      return;
    }
    await _audioRecorder.cancel();
    if (!mounted) {
      return;
    }
    setState(() {
      _recording = false;
      _recordingPath = null;
      _sessionMessage = 'Recording cancelled.';
    });
  }

  Future<void> _clearCanvas() async {
    if (_sessionRunning || _recording || _transcribing || _clearingCanvas) {
      return;
    }
    setState(() {
      _clearingCanvas = true;
      _sessionMessage = 'Clearing canvas...';
    });
    try {
      await _graphSubscription?.cancel();
      _graphSubscription = null;
      await _graphRepository.clear();
      if (!mounted) {
        return;
      }
      _motion.stop();
      _cameraMotion.stop();
      _bridgeFlow.stop();
      _canvasViewport.setView(camera: Offset.zero, zoom: 1);
      const emptyEvents = <ResearchEvent>[];
      const emptyPositions = <String, Offset>{};
      _cacheSettledDisplayLayouts(
        const <String, EventLayout>{},
        events: emptyEvents,
        basePositions: emptyPositions,
        activeId: null,
      );
      setState(() {
        _events = emptyEvents;
        _bridges = const [];
        _basePositions = emptyPositions;
        _progressMessages = const [];
        _sessionGeneratedEventIds = {};
        _sessionFocusEventIds = {};
        _pendingFocusEventIds = null;
        _autoFollowGeneratedCluster = true;
        _activeId = null;
        _hoveredArtifactUrl = null;
        _sessionRunning = false;
        _sessionMessage = 'Canvas cleared.';
      });
    } catch (error) {
      if (!mounted) {
        return;
      }
      setState(() => _sessionMessage = _formatRecordingError(error));
    } finally {
      if (mounted) {
        setState(() => _clearingCanvas = false);
      }
    }
  }

  void _applyGraphState(CanvasGraphState state) {
    if (!mounted) {
      return;
    }

    final previousIds = _events.map((event) => event.id).toSet();
    final newIds = {
      for (final event in state.events)
        if (!previousIds.contains(event.id)) event.id,
    };
    final sessionActive = _sessionRunning || state.isRunning;
    final sessionGeneratedIds = sessionActive
        ? {..._sessionGeneratedEventIds, ...newIds}
        : _sessionGeneratedEventIds;
    final focusEventIds = sessionActive
        ? _focusIdsForSession(state.events, sessionGeneratedIds)
        : <String>{};
    final shouldFocusGeneratedEvents = _autoFollowGeneratedCluster &&
        sessionActive &&
        focusEventIds.isNotEmpty &&
        (newIds.isNotEmpty ||
            _bridgesChanged(_bridges, state.bridges) ||
            (_sessionRunning && !state.isRunning));
    final generated = generateBasePositions(
      state.events,
      bridges: state.bridges,
    );
    final nextPositions = {
      for (final event in state.events) event.id: generated[event.id]!,
    };
    final hasActiveEvent =
        _activeId == null || state.events.any((event) => event.id == _activeId);
    final nextActiveId = hasActiveEvent ? _activeId : null;
    final fromLayouts = _currentLayouts(
      fallback: _settledDisplayLayouts(),
    );
    final toLayouts = displayLayout(
      events: state.events,
      basePositions: nextPositions,
      activeId: nextActiveId,
    );
    _cacheSettledDisplayLayouts(
      toLayouts,
      events: state.events,
      basePositions: nextPositions,
      activeId: nextActiveId,
    );
    final shouldAnimateLayout = _layoutsChanged(fromLayouts, toLayouts);

    _motion.stop();
    if (shouldAnimateLayout) {
      _motionFromLayouts = fromLayouts;
      _motionToLayouts = toLayouts;
      _motionFromActiveId = _activeId;
      _motionToActiveId = nextActiveId;
      _motion.value = 0;
      _setBridgeFlowActive(nextActiveId ?? _activeId);
    } else {
      _motionFromLayouts = null;
      _motionToLayouts = null;
      _motionFromActiveId = null;
      _motionToActiveId = null;
    }

    setState(() {
      _events = state.events;
      _bridges = state.bridges;
      _basePositions = nextPositions;
      _progressMessages = state.progressMessages;
      _sessionMessage = state.error ?? state.message;
      _sessionRunning = state.isRunning;
      _sessionGeneratedEventIds =
          state.isRunning ? sessionGeneratedIds : <String>{};
      _sessionFocusEventIds = state.isRunning ? focusEventIds : <String>{};
      if (!hasActiveEvent) {
        _activeId = null;
        _hoveredArtifactUrl = null;
      }
    });

    if (shouldAnimateLayout) {
      _motion.forward(from: 0);
    }
    if (shouldFocusGeneratedEvents && focusEventIds.isNotEmpty) {
      _pendingFocusEventIds = focusEventIds;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && _pendingFocusEventIds != null) {
          _focusEvents(_pendingFocusEventIds!, _basePositions);
          if (!_motion.isAnimating) {
            _pendingFocusEventIds = null;
          }
        }
      });
    } else {
      _pendingFocusEventIds = null;
    }
  }

  bool _bridgesChanged(List<EventBridge> from, List<EventBridge> to) {
    if (from.length != to.length) {
      return true;
    }
    final previous = {
      for (final bridge in from)
        '${bridge.from}\u0000${bridge.to}\u0000${bridge.label}',
    };
    for (final bridge in to) {
      if (!previous
          .contains('${bridge.from}\u0000${bridge.to}\u0000${bridge.label}')) {
        return true;
      }
    }
    return false;
  }

  Set<String> _focusIdsForSession(
    List<ResearchEvent> events,
    Set<String> generatedIds,
  ) {
    final existing = {for (final event in events) event.id};
    final generated = generatedIds.where(existing.contains).toSet();
    if (generated.isNotEmpty) {
      return generated;
    }
    return _sessionFocusEventIds.where(existing.contains).toSet();
  }

  bool _layoutsChanged(
    Map<String, EventLayout> from,
    Map<String, EventLayout> to,
  ) {
    if (from.length != to.length) {
      return true;
    }
    for (final entry in to.entries) {
      final previous = from[entry.key];
      if (previous == null || previous.display != entry.value.display) {
        return true;
      }
    }
    return false;
  }

  void _focusEvents(Set<String> ids, Map<String, Offset> positions) {
    final viewportSize = _viewportSize;
    if (viewportSize == null) {
      return;
    }
    final transform =
        _CanvasTransform(size: viewportSize, camera: _camera, zoom: _zoom);
    final target = cameraTargetForEvents(
      ids,
      positions,
      visibleWorldSize: Size(
        viewportSize.width / transform.scale,
        viewportSize.height / transform.scale,
      ),
    );
    if (target == null) {
      return;
    }
    if (_debugCamera) {
      debugPrint(
        'Canvas focus: ids=${ids.length} from=$_camera to=$target viewport=$viewportSize',
      );
    }
    _animateCameraTo(target);
  }
}

class _EventCanvasPainter extends CustomPainter {
  _EventCanvasPainter({
    required Listenable repaint,
    required this.events,
    required this.bridges,
    required this.layouts,
    required this.activeId,
    required this.bridgeActiveId,
    required this.hoveredArtifactUrl,
    required this.artifactHover,
    required this.expansionProgresses,
    required this.viewport,
    required this.bridgeFlow,
  }) : super(repaint: repaint);

  final List<ResearchEvent> events;
  final List<EventBridge> bridges;
  final Map<String, EventLayout> layouts;
  final String? activeId;
  final String? bridgeActiveId;
  final String? hoveredArtifactUrl;
  final Animation<double> artifactHover;
  final Map<String, double> expansionProgresses;
  final _CanvasViewportController viewport;
  final Animation<double> bridgeFlow;

  static const _textCacheLimit = 2048;
  static final Map<String, TextPainter> _textCache = {};

  @override
  void paint(Canvas canvas, Size size) {
    final camera = viewport.camera;
    final zoom = viewport.zoom;
    final transform = _CanvasTransform(size: size, camera: camera, zoom: zoom);
    final visibleWorldBounds = Rect.fromPoints(
      transform.screenToWorld(Offset.zero),
      transform.screenToWorld(Offset(size.width, size.height)),
    ).inflate(220 / transform.scale);

    canvas.save();
    canvas.translate(transform.origin.dx, transform.origin.dy);
    canvas.scale(transform.scale);
    canvas.translate(-camera.dx, -camera.dy);

    _paintGrid(canvas, visibleWorldBounds);
    _paintBridges(canvas, visibleWorldBounds);
    _paintEvents(canvas, visibleWorldBounds);

    canvas.restore();
  }

  void _paintGrid(Canvas canvas, Rect visibleWorldBounds) {
    final minor = Paint()
      ..color = _gridLine
      ..strokeWidth = 1;
    final major = Paint()
      ..color = _gridMajor
      ..strokeWidth = 1;
    final startX = (visibleWorldBounds.left / 48).floor() * 48.0;
    final endX = visibleWorldBounds.right;
    final startY = (visibleWorldBounds.top / 48).floor() * 48.0;
    final endY = visibleWorldBounds.bottom;

    for (var x = startX; x <= endX; x += 48) {
      canvas.drawLine(Offset(x, startY), Offset(x, endY), minor);
    }
    for (var y = startY; y <= endY; y += 48) {
      canvas.drawLine(Offset(startX, y), Offset(endX, y), minor);
    }

    const majorStep = 240.0;
    final majorStartX =
        (visibleWorldBounds.left / majorStep).floor() * majorStep;
    final majorStartY =
        (visibleWorldBounds.top / majorStep).floor() * majorStep;
    for (var x = majorStartX; x <= endX; x += majorStep) {
      canvas.drawLine(Offset(x, startY), Offset(x, endY), major);
    }
    for (var y = majorStartY; y <= endY; y += majorStep) {
      canvas.drawLine(Offset(startX, y), Offset(endX, y), major);
    }
  }

  void _paintBridges(Canvas canvas, Rect visibleWorldBounds) {
    for (final bridge in bridges) {
      final from = layouts[bridge.from];
      final to = layouts[bridge.to];
      if (from == null || to == null) {
        continue;
      }
      final bridgeBounds = Rect.fromCircle(center: from.display, radius: 120)
          .expandToInclude(Rect.fromCircle(center: to.display, radius: 120))
          .inflate(96);
      if (!bridgeBounds.overlaps(visibleWorldBounds)) {
        continue;
      }
      final activeProgress = _bridgeProgress(bridge);
      final path = bridgePath(from, to);

      if (activeProgress > 0.01) {
        canvas.drawPath(
          path,
          Paint()
            ..color = _bridgeHi.withValues(alpha: 0.12 * activeProgress)
            ..style = PaintingStyle.stroke
            ..strokeWidth = _lerpDouble(7, 12, activeProgress)
            ..strokeCap = StrokeCap.round
            ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 6),
        );
      }

      final lineColor = Color.lerp(_bridge, _bridgeHi, activeProgress)!;
      final paint = Paint()
        ..color =
            lineColor.withValues(alpha: _lerpDouble(0.22, 0.72, activeProgress))
        ..style = PaintingStyle.stroke
        ..strokeWidth = _lerpDouble(2.2, 3.4, activeProgress)
        ..strokeCap = StrokeCap.round;

      _drawDashedPath(canvas, path, paint, phase: bridgeFlow.value * 140);
    }
  }

  double _bridgeProgress(EventBridge bridge) {
    if (bridgeActiveId == null ||
        (bridge.from != bridgeActiveId && bridge.to != bridgeActiveId)) {
      return 0;
    }
    final progress = expansionProgresses[bridgeActiveId!];
    if (progress != null) {
      return progress.clamp(0.0, 1.0);
    }
    return activeId == bridgeActiveId ? 1 : 0;
  }

  void _paintEvents(Canvas canvas, Rect visibleWorldBounds) {
    for (final event in events) {
      final layout = layouts[event.id]!;
      final openProgress = event.canExpand
          ? (expansionProgresses[event.id] ?? 0).clamp(0.0, 1.0)
          : 0.0;
      if (!_eventPaintBounds(layout, openProgress)
          .overlaps(visibleWorldBounds)) {
        continue;
      }
      final color = Color(event.color);
      final isActive = event.id == activeId;

      canvas.save();
      canvas.translate(layout.display.dx, layout.display.dy);

      if (openProgress > 0) {
        for (final artifact in layout.artifacts) {
          _paintArtifact(
            canvas,
            artifact,
            color,
            openProgress,
            hoveredArtifactUrl == artifact.artifact.url
                ? Curves.easeOutCubic.transform(artifactHover.value)
                : 0,
          );
        }
      }

      final nodeRadius = _lerpDouble(22, 17, openProgress);

      // Soft chromatic halo.
      final glowRadius = nodeRadius * 2.6;
      canvas.drawCircle(
        Offset.zero,
        glowRadius,
        Paint()
          ..shader = RadialGradient(
            colors: [
              color.withValues(alpha: isActive ? 0.34 : 0.22),
              color.withValues(alpha: 0.0),
            ],
          ).createShader(
            Rect.fromCircle(center: Offset.zero, radius: glowRadius),
          ),
      );

      // Marker ring — light hairline, or saturated when selected.
      canvas.drawCircle(
        Offset.zero,
        nodeRadius + 4,
        Paint()
          ..color = (isActive ? color : _inkText)
              .withValues(alpha: isActive ? 0.6 : 0.16)
          ..style = PaintingStyle.stroke
          ..strokeWidth = isActive ? 1.7 : 1.1,
      );

      // Filled node.
      canvas.drawCircle(
        Offset.zero,
        nodeRadius,
        Paint()..color = color,
      );

      // Inner highlight — a small off-center glint for dimensionality.
      canvas.drawCircle(
        Offset(nodeRadius * -0.3, nodeRadius * -0.34),
        nodeRadius * 0.32,
        Paint()
          ..color = Colors.white.withValues(alpha: 0.16)
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 2),
      );

      if (openProgress < 1) {
        final labelOpacity = 1 - openProgress;
        final labelOffset = Offset(0, -10 * openProgress);
        const titleStyle = TextStyle(
          fontFamily: _display,
          color: _inkText,
          fontSize: 15,
          fontWeight: FontWeight.w700,
          height: 1.14,
          letterSpacing: 0.15,
        );
        final titlePainter = _textPainter(event.title, titleStyle, 156);
        final titleTop = 44.0 + labelOffset.dy;
        _drawCenteredText(
          canvas,
          event.title,
          Offset(0, titleTop + titlePainter.height / 2),
          maxWidth: 156,
          style: titleStyle,
          opacity: labelOpacity,
        );
        _drawCenteredText(
          canvas,
          event.date,
          Offset(0, titleTop + titlePainter.height + 13),
          maxWidth: 150,
          style: const TextStyle(
            fontFamily: _mono,
            color: _inkTextDim,
            fontSize: 10,
            fontWeight: FontWeight.w700,
            letterSpacing: 1.2,
          ),
          opacity: labelOpacity,
        );
      }

      canvas.restore();
    }
  }

  Rect _eventPaintBounds(EventLayout layout, double openProgress) {
    final expandedRadius = layout.radius * openProgress;
    final labelRadius = _lerpDouble(124, 28, openProgress);
    return Rect.fromCircle(
      center: layout.display,
      radius: math.max(expandedRadius, labelRadius),
    );
  }

  void _paintArtifact(
    Canvas canvas,
    ArtifactLayout artifact,
    Color color,
    double progress,
    double hoverProgress,
  ) {
    final eased = Curves.easeOutCubic.transform(progress);
    final offset = artifact.offset * eased;
    final alpha = eased.clamp(0.0, 1.0);
    final hoverLift = hoverProgress.clamp(0.0, 1.0).toDouble();
    final radius = artifact.radius * _lerpDouble(0.2, 1, eased) + hoverLift * 2;

    // Tether from node to artifact.
    canvas.drawLine(
      Offset.zero,
      offset,
      Paint()
        ..color = _inkText.withValues(alpha: 0.18 * alpha)
        ..strokeWidth = 1.4,
    );

    // Chromatic halo (brightens on hover).
    if (alpha > 0.02) {
      canvas.drawCircle(
        offset,
        radius + 8 + hoverLift * 4,
        Paint()
          ..shader = RadialGradient(
            colors: [
              color.withValues(alpha: (0.22 + hoverLift * 0.3) * alpha),
              color.withValues(alpha: 0.0),
            ],
          ).createShader(
            Rect.fromCircle(center: offset, radius: radius + 12),
          ),
      );
    }

    // Dark glass fill.
    canvas.drawCircle(
      offset,
      radius,
      Paint()
        ..color = _panelRaised.withValues(alpha: 0.96 * alpha)
        ..style = PaintingStyle.fill,
    );
    // Colored marker ring.
    canvas.drawCircle(
      offset,
      radius,
      Paint()
        ..color = color.withValues(alpha: (0.8 + hoverLift * 0.2) * alpha)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.8 + hoverLift * 1.4,
    );
    // Inner hairline for crispness.
    canvas.drawCircle(
      offset,
      radius - 3,
      Paint()
        ..color = _inkText.withValues(alpha: 0.07 * alpha)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1,
    );

    if (progress > 0.96) {
      final label = _textPainter(
        artifact.lines.join('\n'),
        const TextStyle(
          fontFamily: _mono,
          color: _inkText,
          fontSize: 10,
          fontWeight: FontWeight.w700,
          height: 1.12,
          letterSpacing: 0.3,
        ),
        artifact.radius * 1.65,
      );
      canvas.drawTextPainter(
        label,
        offset - Offset(label.width / 2, label.height / 2 + 1),
      );
    }
  }

  void _drawCenteredText(
    Canvas canvas,
    String text,
    Offset center, {
    required double maxWidth,
    required TextStyle style,
    double opacity = 1,
  }) {
    if (opacity <= 0.02) {
      return;
    }
    final fillParagraph = _textPainter(
      text,
      style.copyWith(
          color:
              style.color?.withValues(alpha: (style.color?.a ?? 1) * opacity)),
      maxWidth,
    );
    final origin =
        center - Offset(fillParagraph.width / 2, fillParagraph.height / 2);
    canvas.drawTextPainter(fillParagraph, origin);
  }

  TextPainter _textPainter(String text, TextStyle style, double maxWidth) {
    final key = Object.hash(
      text,
      style.fontFamily,
      style.color?.toARGB32(),
      style.fontSize,
      style.fontWeight,
      style.fontStyle,
      style.height,
      style.letterSpacing,
      maxWidth,
      style.foreground?.color.toARGB32(),
    ).toString();
    final cached = _textCache[key];
    if (cached != null) {
      return cached;
    }
    final painter = TextPainter(
      text: TextSpan(text: text, style: style),
      textAlign: TextAlign.center,
      textDirection: TextDirection.ltr,
    )..layout(maxWidth: maxWidth);
    if (_textCache.length >= _textCacheLimit) {
      _textCache.clear();
    }
    _textCache[key] = painter;
    return painter;
  }

  void _drawDashedPath(
    Canvas canvas,
    Path path,
    Paint paint, {
    required double phase,
  }) {
    const dash = 8.0;
    const gap = 12.0;
    final interval = dash + gap;

    for (final metric in path.computeMetrics()) {
      var distance = -(phase % interval);
      while (distance < metric.length) {
        final start = math.max(0.0, distance);
        final end = math.min(metric.length, distance + dash);
        if (end > start) {
          canvas.drawPath(metric.extractPath(start, end), paint);
        }
        distance += interval;
      }
    }
  }

  @override
  bool shouldRepaint(_EventCanvasPainter oldDelegate) {
    return oldDelegate.layouts != layouts ||
        oldDelegate.activeId != activeId ||
        oldDelegate.bridgeActiveId != bridgeActiveId ||
        oldDelegate.hoveredArtifactUrl != hoveredArtifactUrl ||
        oldDelegate.artifactHover != artifactHover ||
        oldDelegate.expansionProgresses != expansionProgresses ||
        oldDelegate.viewport != viewport ||
        oldDelegate.bridgeFlow != bridgeFlow;
  }
}

class CanvasBackgroundPainter extends CustomPainter {
  const CanvasBackgroundPainter();

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Offset.zero & size;
    canvas.drawRect(
      rect,
      Paint()
        ..shader = const LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [_ink0, _ink1, _ink2],
          stops: [0.0, 0.55, 1.0],
        ).createShader(rect),
    );

    // Signal glows — warm amber (top-left), violet (upper-right), cool cyan
    // (lower-right). Atmospheric depth rather than a flat fill.
    final glows = <(Offset, Color, double)>[
      (const Offset(0.14, 0.12), _signal.withValues(alpha: 0.21), 0.62),
      (const Offset(0.86, 0.18), _plum.withValues(alpha: 0.16), 0.58),
      (const Offset(0.80, 0.88), _data.withValues(alpha: 0.15), 0.66),
    ];
    final glowPaint = Paint();
    for (final glow in glows) {
      final center =
          Offset(size.width * glow.$1.dx, size.height * glow.$1.dy);
      final radius = math.max(size.width, size.height) * glow.$3;
      canvas.drawCircle(
        center,
        radius,
        glowPaint
          ..shader = RadialGradient(
            colors: [glow.$2, Colors.transparent],
          ).createShader(Rect.fromCircle(center: center, radius: radius)),
      );
    }

    // Vignette — sink the edges into ink so the canvas reads as a lit pool.
    canvas.drawRect(
      rect,
      Paint()
        ..shader = const RadialGradient(
          center: Alignment(0.0, -0.06),
          radius: 1.38,
          colors: [Color(0x0006090f), Color(0x5406090f), _ink0],
          stops: [0.0, 0.62, 1.0],
        ).createShader(rect),
    );

    // Static grain — a faint pinpoint field for texture (screen-space, once).
    final grain = <Offset>[];
    var state = 2654435769;
    double rnd() {
      state = (state * 1664525 + 1013904223) & 0x7fffffff;
      return state / 2147483647.0;
    }
    const spacing = 50.0;
    for (var y = -spacing; y < size.height + spacing; y += spacing) {
      for (var x = -spacing; x < size.width + spacing; x += spacing) {
        grain.add(Offset(x + (rnd() - 0.5) * 30, y + (rnd() - 0.5) * 30));
      }
    }
    canvas.drawPoints(
      ui.PointMode.points,
      grain,
      Paint()
        ..color = const Color(0x0cffffff)
        ..strokeWidth = 1.25
        ..strokeCap = StrokeCap.round,
    );
  }

  @override
  bool shouldRepaint(CanvasBackgroundPainter oldDelegate) {
    return false;
  }
}

double _lerpDouble(double from, double to, double progress) {
  return from + (to - from) * progress;
}

class _MetadataSheet extends StatelessWidget {
  const _MetadataSheet({
    required this.layout,
    required this.viewportSize,
  });

  final EventLayout layout;
  final Size viewportSize;

  @override
  Widget build(BuildContext context) {
    final mobile = viewportSize.width < 720;
    final leftSide = layout.display.dx >= canvasSize.width * 0.5;
    final highSide = layout.display.dy > canvasSize.height * 0.52;
    final eventColor = Color(layout.event.color);
    final desktopWidth = switch (layout.event.summary.length) {
      < 145 => 268.0,
      < 230 => 312.0,
      _ => 352.0,
    };

    final sheet = DecoratedBox(
      decoration: BoxDecoration(
        color: _panel,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: _hairline),
        boxShadow: [
          BoxShadow(
            color: _ink0.withValues(alpha: 0.62),
            blurRadius: 36,
            offset: const Offset(0, 18),
          ),
          BoxShadow(
            color: eventColor.withValues(alpha: 0.2),
            blurRadius: 24,
            offset: const Offset(0, 6),
          ),
        ],
      ),
      child: SizedBox(
        width: mobile ? viewportSize.width - 28 : desktopWidth,
        child: ConstrainedBox(
          constraints: BoxConstraints(
            maxHeight: math.min(260, viewportSize.height - 150),
          ),
          child: ClipRRect(
            borderRadius: BorderRadius.circular(10),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                DecoratedBox(
                  decoration: BoxDecoration(
                    color: eventColor,
                    boxShadow: [
                      BoxShadow(
                        color: eventColor.withValues(alpha: 0.5),
                        blurRadius: 18,
                      ),
                    ],
                  ),
                  child: const SizedBox(width: 4),
                ),
                Expanded(
                  child: SingleChildScrollView(
                    padding: const EdgeInsets.fromLTRB(14, 12, 14, 13),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            Flexible(
                              child: Text(
                                layout.event.sourceLabel.toUpperCase(),
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: const TextStyle(
                                  fontFamily: _mono,
                                  color: _inkTextDim,
                                  fontSize: 9.5,
                                  fontWeight: FontWeight.w700,
                                  letterSpacing: 1.4,
                                ),
                              ),
                            ),
                            const SizedBox(width: 8),
                            Text(
                              layout.event.date,
                              style: const TextStyle(
                                fontFamily: _mono,
                                color: _inkTextFaint,
                                fontSize: 9.5,
                                fontWeight: FontWeight.w600,
                                letterSpacing: 0.8,
                              ),
                            ),
                          ],
                        ),
                        const SizedBox(height: 8),
                        Text(
                          layout.event.summary,
                          style: const TextStyle(
                            fontFamily: _display,
                            color: _inkText,
                            fontSize: 13.5,
                            height: 1.5,
                            fontWeight: FontWeight.w500,
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );

    if (mobile) {
      return Positioned(
        left: 14,
        right: 14,
        bottom: 92,
        child: sheet,
      );
    }

    return Positioned(
      left: leftSide ? 22 : null,
      right: leftSide ? null : 22,
      top: highSide ? 78 : null,
      bottom: highSide ? null : 22,
      child: sheet,
    );
  }
}

class _GlassPanel extends StatelessWidget {
  const _GlassPanel({required this.child, this.pill = false});

  final Widget child;
  final bool pill;

  @override
  Widget build(BuildContext context) {
    return Material(
      color: _panel,
      elevation: 12,
      shadowColor: _ink0,
      surfaceTintColor: Colors.transparent,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(pill ? 999 : 10),
        side: const BorderSide(color: _hairline, width: 1),
      ),
      child: child,
    );
  }
}

class _SessionStatus extends StatelessWidget {
  const _SessionStatus({
    required this.message,
    required this.running,
  });

  final String message;
  final bool running;

  @override
  Widget build(BuildContext context) {
    return Positioned(
      left: 22,
      right: 22,
      bottom: MediaQuery.sizeOf(context).width > 720 ? 28 : 90,
      child: IgnorePointer(
        child: Align(
          alignment: Alignment.bottomLeft,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 420),
            child: _GlassPanel(
              child: Padding(
                padding:
                    const EdgeInsets.symmetric(horizontal: 13, vertical: 11),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    SizedBox(
                      width: 9,
                      height: 9,
                      child: DecoratedBox(
                        decoration: BoxDecoration(
                          color: running ? _signalHot : _data,
                          shape: BoxShape.circle,
                          boxShadow: [
                            BoxShadow(
                              color: (running ? _signalHot : _data)
                                  .withValues(alpha: 0.6),
                              blurRadius: 8,
                            ),
                          ],
                        ),
                      ),
                    ),
                    const SizedBox(width: 9),
                    Text(
                      running ? 'LIVE' : 'IDLE',
                      style: TextStyle(
                        fontFamily: _mono,
                        color: running ? _signalHot : _data,
                        fontSize: 9.5,
                        fontWeight: FontWeight.w700,
                        letterSpacing: 1.4,
                      ),
                    ),
                    const SizedBox(width: 8),
                    Flexible(
                      child: Text(
                        message,
                        maxLines: 3,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          fontFamily: _mono,
                          color: _inkText,
                          fontSize: 11,
                          height: 1.3,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _HermesActivityDrawer extends StatelessWidget {
  const _HermesActivityDrawer({
    required this.messages,
    required this.running,
    required this.open,
    required this.onToggle,
  });

  final List<String> messages;
  final bool running;
  final bool open;
  final VoidCallback onToggle;

  @override
  Widget build(BuildContext context) {
    final viewport = MediaQuery.sizeOf(context);
    final panelWidth = viewport.width > 960 ? 380.0 : viewport.width - 92;
    final top = viewport.width > 960 ? 72.0 : 84.0;
    final bottom = viewport.width > 960 ? 116.0 : 164.0;
    final width = panelWidth.clamp(280.0, 380.0).toDouble();

    return Positioned(
      top: top,
      right: 14,
      bottom: bottom,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Align(
            alignment: Alignment.topRight,
            child: Tooltip(
              message: open ? 'Close Hermes activity' : 'Open Hermes activity',
              child: _GlassPanel(
                pill: true,
                child: IconButton(
                  icon: Icon(
                    open ? Icons.chevron_right : Icons.chevron_left,
                    color: _inkTextDim,
                    size: 20,
                  ),
                  onPressed: onToggle,
                ),
              ),
            ),
          ),
          ClipRect(
            child: AnimatedContainer(
              duration: const Duration(milliseconds: 180),
              curve: Curves.easeOutCubic,
              width: open ? width : 0,
              child: Padding(
                padding: const EdgeInsets.only(left: 8),
                child: SizedBox(
                  width: width,
                  child: _GlassPanel(
                    child: Padding(
                      padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Row(
                            children: [
                              SizedBox(
                                width: 9,
                                height: 9,
                                child: DecoratedBox(
                                  decoration: BoxDecoration(
                                    color: running ? _signalHot : _data,
                                    shape: BoxShape.circle,
                                    boxShadow: [
                                      BoxShadow(
                                        color: (running
                                                ? _signalHot
                                                : _data)
                                            .withValues(alpha: 0.6),
                                        blurRadius: 8,
                                      ),
                                    ],
                                  ),
                                ),
                              ),
                              const SizedBox(width: 9),
                              const Text(
                                'HERMES',
                                style: TextStyle(
                                  fontFamily: _mono,
                                  color: _inkText,
                                  fontSize: 11,
                                  fontWeight: FontWeight.w700,
                                  letterSpacing: 1.6,
                                ),
                              ),
                              const Spacer(),
                              Text(
                                running ? 'ACTIVE' : 'IDLE',
                                style: const TextStyle(
                                  fontFamily: _mono,
                                  color: _inkTextFaint,
                                  fontSize: 9.5,
                                  fontWeight: FontWeight.w700,
                                  letterSpacing: 1.4,
                                ),
                              ),
                            ],
                          ),
                          const SizedBox(height: 9),
                          Expanded(
                            child: messages.isEmpty
                                ? const Align(
                                    alignment: Alignment.topLeft,
                                    child: Text(
                                      'Awaiting research activity\u2026',
                                      style: TextStyle(
                                        fontFamily: _mono,
                                        color: _inkTextFaint,
                                        fontSize: 11,
                                        height: 1.35,
                                        fontWeight: FontWeight.w600,
                                      ),
                                    ),
                                  )
                                : ListView.separated(
                                    padding: EdgeInsets.zero,
                                    itemCount: messages.length,
                                    separatorBuilder: (_, __) =>
                                        const SizedBox(height: 8),
                                    itemBuilder: (context, index) {
                                      final message = messages[index];
                                      final latest =
                                          index == messages.length - 1;
                                      return DecoratedBox(
                                        decoration: BoxDecoration(
                                          color: latest
                                              ? _signal
                                                  .withValues(alpha: 0.12)
                                              : _panelRaised
                                                  .withValues(alpha: 0.5),
                                          border: Border.all(
                                            color: latest
                                                ? _signal
                                                    .withValues(alpha: 0.5)
                                                : _hairlineDim,
                                          ),
                                          borderRadius:
                                              BorderRadius.circular(6),
                                        ),
                                        child: Padding(
                                          padding: const EdgeInsets.symmetric(
                                            horizontal: 10,
                                            vertical: 8,
                                          ),
                                          child: Text(
                                            message,
                                            style: const TextStyle(
                                              fontFamily: _mono,
                                              color: _inkText,
                                              fontSize: 11,
                                              height: 1.35,
                                              fontWeight: FontWeight.w600,
                                            ),
                                          ),
                                        ),
                                      );
                                    },
                                  ),
                          ),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _RecordButton extends StatelessWidget {
  const _RecordButton({
    required this.running,
    required this.recording,
    required this.transcribing,
    required this.onPressed,
    required this.onCancel,
  });

  final bool running;
  final bool recording;
  final bool transcribing;
  final VoidCallback onPressed;
  final VoidCallback onCancel;

  @override
  Widget build(BuildContext context) {
    final disabled = running && !recording;
    final label = recording
        ? 'Tap to send recording'
        : transcribing
            ? 'Transcribing recording'
            : disabled
                ? 'Research session running'
                : 'Tap to record';
    final orbColor = running
        ? _signalHotDeep
        : (recording || transcribing ? _signalHot : _signal);
    return Positioned(
      left: MediaQuery.sizeOf(context).width / 2 - 36,
      bottom: MediaQuery.sizeOf(context).width > 720 ? 28 : 18,
      child: Semantics(
        button: true,
        label: label,
        child: GestureDetector(
          onTap: disabled ? null : onPressed,
          onLongPress: recording ? onCancel : null,
          child: DecoratedBox(
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: orbColor,
              border: Border.all(
                color: _ink0.withValues(alpha: 0.28),
                width: 2,
              ),
              boxShadow: [
                BoxShadow(
                  color: orbColor.withValues(alpha: 0.55),
                  blurRadius: 36,
                  offset: const Offset(0, 18),
                ),
                BoxShadow(
                  color: orbColor.withValues(alpha: 0.3),
                  blurRadius: 12,
                  offset: const Offset(0, 4),
                ),
              ],
            ),
            child: SizedBox(
              width: MediaQuery.sizeOf(context).width > 720 ? 72 : 64,
              height: MediaQuery.sizeOf(context).width > 720 ? 72 : 64,
              child: Center(
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    color: _ink0,
                    shape: running || recording || transcribing
                        ? BoxShape.rectangle
                        : BoxShape.circle,
                    borderRadius: running || recording || transcribing
                        ? BorderRadius.circular(4)
                        : null,
                  ),
                  child: SizedBox(
                    width: running || recording || transcribing ? 18 : 17,
                    height: running || recording || transcribing ? 18 : 17,
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _ZoomControls extends StatelessWidget {
  const _ZoomControls({
    required this.zoom,
    required this.onZoomIn,
    required this.onZoomOut,
    required this.onReset,
    required this.clearing,
    required this.clearEnabled,
    required this.onClear,
  });

  final ValueListenable<double> zoom;
  final VoidCallback onZoomIn;
  final VoidCallback onZoomOut;
  final VoidCallback onReset;
  final bool clearing;
  final bool clearEnabled;
  final VoidCallback onClear;

  @override
  Widget build(BuildContext context) {
    return Positioned(
      right: 18,
      bottom: MediaQuery.sizeOf(context).width > 720 ? 28 : 18,
        child: DecoratedBox(
          decoration: BoxDecoration(
            color: _panel,
            border: Border.all(color: _hairline),
            borderRadius: BorderRadius.circular(10),
            boxShadow: const [
              BoxShadow(
                color: Color(0x82000000),
                blurRadius: 24,
                offset: Offset(0, 10),
              ),
            ],
          ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            _ZoomIconButton(
              icon: Icons.remove,
              tooltip: 'Zoom out',
              onPressed: onZoomOut,
            ),
            SizedBox(
              width: 54,
              child: Center(
                child: ValueListenableBuilder<double>(
                  valueListenable: zoom,
                  builder: (context, value, _) {
                    return Text(
                      '${(value * 100).round()}%',
                      style: const TextStyle(
                        fontFamily: _mono,
                        color: _inkText,
                        fontSize: 11,
                        fontWeight: FontWeight.w700,
                        letterSpacing: 0.4,
                      ),
                    );
                  },
                ),
              ),
            ),
            _ZoomIconButton(
              icon: Icons.add,
              tooltip: 'Zoom in',
              onPressed: onZoomIn,
            ),
            _ZoomIconButton(
              icon: Icons.center_focus_strong,
              tooltip: 'Reset zoom',
              onPressed: onReset,
            ),
            const SizedBox(
              height: 28,
              child: VerticalDivider(
                width: 1,
                thickness: 1,
                color: _hairline,
              ),
            ),
            _ZoomIconButton(
              icon: clearing ? Icons.more_horiz : Icons.delete_outline,
              tooltip: 'Clear canvas',
              onPressed: clearEnabled ? onClear : null,
            ),
          ],
        ),
      ),
    );
  }
}

class _ZoomIconButton extends StatelessWidget {
  const _ZoomIconButton({
    required this.icon,
    required this.tooltip,
    required this.onPressed,
  });

  final IconData icon;
  final String tooltip;
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      child: IconButton(
        visualDensity: VisualDensity.compact,
        iconSize: 18,
        color: _inkTextDim,
        onPressed: onPressed,
        icon: Icon(icon),
      ),
    );
  }
}

class _CanvasViewportController extends ChangeNotifier {
  _CanvasViewportController() : zoomListenable = ValueNotifier<double>(1);

  final ValueNotifier<double> zoomListenable;
  Offset _camera = Offset.zero;
  double _zoom = 1;

  Offset get camera => _camera;

  double get zoom => _zoom;

  void setCamera(Offset camera) {
    if (_camera == camera) {
      return;
    }
    _camera = camera;
    notifyListeners();
  }

  void setView({
    required Offset camera,
    required double zoom,
  }) {
    final cameraChanged = _camera != camera;
    final zoomChanged = _zoom != zoom;
    if (!cameraChanged && !zoomChanged) {
      return;
    }

    _camera = camera;
    if (zoomChanged) {
      _zoom = zoom;
      zoomListenable.value = zoom;
    }
    notifyListeners();
  }

  @override
  void dispose() {
    zoomListenable.dispose();
    super.dispose();
  }
}

class _CanvasTransform {
  const _CanvasTransform({
    required this.size,
    required this.camera,
    required this.zoom,
  });

  final Size size;
  final Offset camera;
  final double zoom;

  double get scale =>
      math.min(size.width / canvasSize.width, size.height / canvasSize.height) *
      zoom;

  Offset get origin => Offset(
        (size.width - canvasSize.width * scale) / 2,
        (size.height - canvasSize.height * scale) / 2,
      );

  Offset screenToWorld(Offset screen) {
    return Offset(
      (screen.dx - origin.dx) / scale + camera.dx,
      (screen.dy - origin.dy) / scale + camera.dy,
    );
  }
}

extension on Canvas {
  void drawTextPainter(TextPainter painter, Offset offset) {
    painter.paint(this, offset);
  }
}
