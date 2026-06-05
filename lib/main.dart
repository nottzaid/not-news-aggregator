import 'dart:async';
import 'dart:math' as math;

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
        fontFamily: 'Inter',
        colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xffb85534)),
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
  late Map<String, Offset> _basePositions;
  List<ResearchEvent> _events = fixtureEvents;
  List<EventBridge> _bridges = fixtureBridges;
  List<String> _progressMessages = const [];
  bool _hermesPanelOpen = false;
  StreamSubscription<CanvasGraphState>? _graphSubscription;
  Offset _camera = Offset.zero;
  double _zoom = 1;
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
        setState(
            () => _camera = _clampedCamera(Offset.lerp(from, to, progress)!));
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
    _motion.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final targetLayouts = displayLayout(
      events: _events,
      basePositions: _basePositions,
      activeId: _activeId,
    );
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
                            painter: EventCanvasPainter(
                              repaint: Listenable.merge(
                                  [_bridgeFlow, _artifactHover]),
                              events: _events,
                              bridges: _bridges,
                              layouts: animatedLayouts,
                              activeId: _activeId,
                              bridgeActiveId: _bridgeActiveId,
                              hoveredArtifactUrl: _hoveredArtifactUrl,
                              artifactHover: _artifactHover,
                              expansionProgresses: expansionProgresses,
                              camera: _camera,
                              zoom: _zoom,
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
                zoom: _zoom,
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
    setState(() => _camera = _clampedCamera(camera));
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

    setState(() {
      _zoom = nextZoom;
      _camera = _clampedCamera(nextCamera);
    });
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
      fallback: displayLayout(
        events: _events,
        basePositions: _basePositions,
        activeId: _activeId,
      ),
    );
    final previousId = _activeId;
    final to = displayLayout(
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
      fallback: displayLayout(
        events: _events,
        basePositions: _basePositions,
        activeId: _activeId,
      ),
    );
  }

  Map<String, double> _expansionProgresses() {
    if (_motionFromActiveId == null && _motionToActiveId == null) {
      return {
        if (_activeId != null) _activeId!: 1,
      };
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
      setState(() {
        _events = const [];
        _bridges = const [];
        _basePositions = const {};
        _progressMessages = const [];
        _sessionGeneratedEventIds = {};
        _sessionFocusEventIds = {};
        _pendingFocusEventIds = null;
        _autoFollowGeneratedCluster = true;
        _activeId = null;
        _hoveredArtifactUrl = null;
        _camera = Offset.zero;
        _zoom = 1;
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
      fallback: displayLayout(
        events: _events,
        basePositions: _basePositions,
        activeId: _activeId,
      ),
    );
    final toLayouts = displayLayout(
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

class EventCanvasPainter extends CustomPainter {
  EventCanvasPainter({
    required Listenable repaint,
    required this.events,
    required this.bridges,
    required this.layouts,
    required this.activeId,
    required this.bridgeActiveId,
    required this.hoveredArtifactUrl,
    required this.artifactHover,
    required this.expansionProgresses,
    required this.camera,
    required this.zoom,
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
  final Offset camera;
  final double zoom;
  final Animation<double> bridgeFlow;

  static final Map<String, TextPainter> _textCache = {};

  @override
  void paint(Canvas canvas, Size size) {
    final transform = _CanvasTransform(size: size, camera: camera, zoom: zoom);
    canvas.save();
    canvas.translate(transform.origin.dx, transform.origin.dy);
    canvas.scale(transform.scale);
    canvas.translate(-camera.dx, -camera.dy);

    _paintGrid(canvas);
    _paintBridges(canvas);
    _paintEvents(canvas);

    canvas.restore();
  }

  void _paintGrid(Canvas canvas) {
    final paint = Paint()
      ..color = const Color(0x09171514)
      ..strokeWidth = 1;
    final startX = ((camera.dx - 720) / 48).floor() * 48.0;
    final endX = camera.dx + canvasSize.width + 720;
    final startY = ((camera.dy - 720) / 48).floor() * 48.0;
    final endY = camera.dy + canvasSize.height + 720;

    for (var x = startX; x <= endX; x += 48) {
      canvas.drawLine(
        Offset(x, startY),
        Offset(x, endY),
        paint,
      );
    }
    for (var y = startY; y <= endY; y += 48) {
      canvas.drawLine(
        Offset(startX, y),
        Offset(endX, y),
        paint,
      );
    }
  }

  void _paintBridges(Canvas canvas) {
    for (final bridge in bridges) {
      final from = layouts[bridge.from];
      final to = layouts[bridge.to];
      if (from == null || to == null) {
        continue;
      }
      final activeProgress = _bridgeProgress(bridge);
      final paint = Paint()
        ..color = const Color(0xff29313a).withValues(
          alpha: _lerpDouble(0.18, 0.62, activeProgress),
        )
        ..style = PaintingStyle.stroke
        ..strokeWidth = _lerpDouble(2.4, 3.2, activeProgress)
        ..strokeCap = StrokeCap.round;

      final path = bridgePath(from, to);
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

  void _paintEvents(Canvas canvas) {
    for (final event in events) {
      final layout = layouts[event.id]!;
      final openProgress = event.canExpand
          ? (expansionProgresses[event.id] ?? 0).clamp(0.0, 1.0)
          : 0.0;
      final color = Color(event.color);

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

      canvas.drawCircle(
        Offset.zero,
        _lerpDouble(22, 17, openProgress),
        Paint()
          ..color = color
          ..style = PaintingStyle.fill,
      );
      if (openProgress < 1) {
        final labelOpacity = 1 - openProgress;
        final labelOffset = Offset(0, -10 * openProgress);
        final titlePainter = _textPainter(
          event.title,
          const TextStyle(
            color: Color(0xff171514),
            fontSize: 14,
            fontWeight: FontWeight.w800,
          ).copyWith(
            color: const Color(0xff171514).withValues(alpha: labelOpacity),
          ),
          150,
        );
        final titleTop = 42.0 + labelOffset.dy;
        _drawCenteredText(
          canvas,
          event.title,
          Offset(0, titleTop + titlePainter.height / 2),
          maxWidth: 150,
          style: const TextStyle(
            color: Color(0xff171514),
            fontSize: 14,
            fontWeight: FontWeight.w800,
          ),
          stroke: const Color(0xf2fffbf3),
          opacity: labelOpacity,
        );
        _drawCenteredText(
          canvas,
          event.date,
          Offset(0, titleTop + titlePainter.height + 12 + 5.5),
          maxWidth: 140,
          style: const TextStyle(
            color: Color(0xff5f5851),
            fontSize: 11,
            fontWeight: FontWeight.w700,
          ),
          stroke: const Color(0xf2fffbf3),
          opacity: labelOpacity,
        );
      }

      canvas.restore();
    }
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
    canvas.drawLine(
      Offset.zero,
      offset,
      Paint()
        ..color = const Color(0xff171514).withValues(alpha: 0.2 * alpha)
        ..strokeWidth = 2,
    );
    canvas.drawCircle(
      offset,
      artifact.radius * _lerpDouble(0.2, 1, eased) + hoverLift * 2,
      Paint()
        ..color = const Color(0xfffffaf1).withValues(alpha: 0.96 * alpha)
        ..style = PaintingStyle.fill,
    );
    canvas.drawCircle(
      offset,
      artifact.radius * _lerpDouble(0.2, 1, eased) + hoverLift * 2,
      Paint()
        ..color = color.withValues(alpha: (0.72 + hoverLift * 0.28) * alpha)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2 + hoverLift * 1.2,
    );
    if (progress > 0.96) {
      final label = _textPainter(
        artifact.lines.join('\n'),
        const TextStyle(
          color: Color(0xff1d1916),
          fontSize: 11,
          fontWeight: FontWeight.w800,
          height: 1.1,
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
    required Color stroke,
    double opacity = 1,
  }) {
    if (opacity <= 0.02) {
      return;
    }
    final strokeParagraph = _textPainter(
      text,
      style.copyWith(
        foreground: Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 5
          ..color = stroke.withValues(alpha: stroke.a * opacity),
      ),
      maxWidth,
    );
    final fillParagraph = _textPainter(
      text,
      style.copyWith(
          color:
              style.color?.withValues(alpha: (style.color?.a ?? 1) * opacity)),
      maxWidth,
    );
    final origin =
        center - Offset(strokeParagraph.width / 2, strokeParagraph.height / 2);
    canvas.drawTextPainter(strokeParagraph, origin);
    canvas.drawTextPainter(fillParagraph, origin);
  }

  TextPainter _textPainter(String text, TextStyle style, double maxWidth) {
    final key = Object.hash(
      text,
      style.color?.toARGB32(),
      style.fontSize,
      style.fontWeight,
      style.height,
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
    if (_textCache.length > 96) {
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
  bool shouldRepaint(EventCanvasPainter oldDelegate) {
    return oldDelegate.layouts != layouts ||
        oldDelegate.activeId != activeId ||
        oldDelegate.bridgeActiveId != bridgeActiveId ||
        oldDelegate.hoveredArtifactUrl != hoveredArtifactUrl ||
        oldDelegate.artifactHover != artifactHover ||
        oldDelegate.expansionProgresses != expansionProgresses ||
        oldDelegate.camera != camera ||
        oldDelegate.zoom != zoom ||
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
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [Color(0xfffffaf2), Color(0xfff8f3e9), Color(0xffedf4ef)],
          stops: [0, 0.52, 1],
        ).createShader(rect),
    );

    for (final glow in const [
      (Offset(0.18, 0.18), Color(0x22b85534), 420.0),
      (Offset(0.78, 0.14), Color(0x1f3169a8), 390.0),
      (Offset(0.78, 0.78), Color(0x1d1f6f60), 390.0),
    ]) {
      final center = Offset(size.width * glow.$1.dx, size.height * glow.$1.dy);
      canvas.drawCircle(
        center,
        glow.$3,
        Paint()
          ..shader = RadialGradient(
            colors: [glow.$2, Colors.transparent],
          ).createShader(
            Rect.fromCircle(center: center, radius: glow.$3),
          ),
      );
    }
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
        color: const Color(0xf8fffcf6),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: const Color(0x2a171514)),
        boxShadow: const [
          BoxShadow(
            color: Color(0x26171514),
            blurRadius: 28,
            offset: Offset(0, 14),
          ),
        ],
      ),
      child: SizedBox(
        width: mobile ? viewportSize.width - 28 : desktopWidth,
        child: ConstrainedBox(
          constraints: BoxConstraints(
            maxHeight: math.min(240, viewportSize.height - 150),
          ),
          child: ClipRRect(
            borderRadius: BorderRadius.circular(8),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                DecoratedBox(
                  decoration: BoxDecoration(
                    color: eventColor.withValues(alpha: 0.54),
                    boxShadow: [
                      BoxShadow(
                        color: eventColor.withValues(alpha: 0.15),
                        blurRadius: 16,
                      ),
                    ],
                  ),
                  child: const SizedBox(width: 5),
                ),
                Expanded(
                  child: SingleChildScrollView(
                    padding: const EdgeInsets.fromLTRB(13, 11, 13, 12),
                    child: Text(
                      layout.event.summary,
                      style: const TextStyle(
                        color: Color(0xff2f2924),
                        fontSize: 12,
                        height: 1.45,
                        fontWeight: FontWeight.w500,
                      ),
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
      color: const Color(0xf8fffcf6),
      elevation: 9,
      shadowColor: const Color(0x17171514),
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(pill ? 999 : 8),
        side: const BorderSide(color: Color(0x26171514)),
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
                    const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    SizedBox(
                      width: 10,
                      height: 10,
                      child: DecoratedBox(
                        decoration: BoxDecoration(
                          color: running
                              ? const Color(0xffd94332)
                              : const Color(0xff1f6f60),
                          shape: BoxShape.circle,
                        ),
                      ),
                    ),
                    const SizedBox(width: 9),
                    Flexible(
                      child: Text(
                        message,
                        maxLines: 3,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          color: Color(0xff27221f),
                          fontSize: 12,
                          height: 1.3,
                          fontWeight: FontWeight.w700,
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
                    color: const Color(0xff27221f),
                    size: 21,
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
                                    color: running
                                        ? const Color(0xffd94332)
                                        : const Color(0xff1f6f60),
                                    shape: BoxShape.circle,
                                  ),
                                ),
                              ),
                              const SizedBox(width: 8),
                              const Text(
                                'Hermes',
                                style: TextStyle(
                                  color: Color(0xff27221f),
                                  fontSize: 12,
                                  fontWeight: FontWeight.w900,
                                ),
                              ),
                              const Spacer(),
                              Text(
                                running ? 'active' : 'idle',
                                style: const TextStyle(
                                  color: Color(0xff6b625a),
                                  fontSize: 10,
                                  fontWeight: FontWeight.w800,
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
                                      'Waiting for research activity.',
                                      style: TextStyle(
                                        color: Color(0xff6b625a),
                                        fontSize: 11,
                                        height: 1.35,
                                        fontWeight: FontWeight.w700,
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
                                              ? const Color(0x1fd94332)
                                              : const Color(0x14ffffff),
                                          border: Border.all(
                                            color: latest
                                                ? const Color(0x45d94332)
                                                : const Color(0x1f171514),
                                          ),
                                          borderRadius:
                                              BorderRadius.circular(6),
                                        ),
                                        child: Padding(
                                          padding: const EdgeInsets.symmetric(
                                            horizontal: 9,
                                            vertical: 8,
                                          ),
                                          child: Text(
                                            message,
                                            style: const TextStyle(
                                              color: Color(0xff27221f),
                                              fontSize: 11,
                                              height: 1.35,
                                              fontWeight: FontWeight.w700,
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
              color:
                  running ? const Color(0xff8f3128) : const Color(0xffd94332),
              boxShadow: const [
                BoxShadow(
                  color: Color(0x45d94332),
                  blurRadius: 34,
                  offset: Offset(0, 18),
                ),
              ],
            ),
            child: SizedBox(
              width: MediaQuery.sizeOf(context).width > 720 ? 72 : 64,
              height: MediaQuery.sizeOf(context).width > 720 ? 72 : 64,
              child: Center(
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    color: const Color(0xfffff8ee),
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

  final double zoom;
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
          color: const Color(0xfffffbf3),
          border: Border.all(color: const Color(0x1f171514)),
          borderRadius: BorderRadius.circular(8),
          boxShadow: const [
            BoxShadow(
              color: Color(0x1f171514),
              blurRadius: 18,
              offset: Offset(0, 8),
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
                child: Text(
                  '${(zoom * 100).round()}%',
                  style: const TextStyle(
                    color: Color(0xff332f2b),
                    fontSize: 12,
                    fontWeight: FontWeight.w800,
                  ),
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
                color: Color(0x1f171514),
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
        color: const Color(0xff332f2b),
        onPressed: onPressed,
        icon: Icon(icon),
      ),
    );
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
