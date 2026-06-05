import '../models/research_event.dart';

const fixtureEvents = <ResearchEvent>[
  ResearchEvent(
    id: 'spacex',
    title: 'SpaceX compute partnership',
    date: 'May 6, 2026',
    color: 0xffb85534,
    summary:
        "Anthropic announces access to SpaceX's Colossus 1 capacity. Claude usage-limit changes and orbital compute interest live inside this event graph as claims, not separate Canvas points.",
    sourceLabel: 'Anthropic',
    artifacts: [
      SourceArtifact(
        text: 'Anthropic announcement',
        source: 'official',
        url: 'https://www.anthropic.com/news/higher-limits-spacex',
      ),
      SourceArtifact(
        text: 'xAI announcement',
        source: 'official',
        url: 'https://x.ai/news/anthropic-compute-partnership',
      ),
      SourceArtifact(
        text: 'Bloomberg report',
        source: 'report',
        url:
            'https://www.bloomberg.com/news/articles/2026-05-06/anthropic-inks-computing-deal-with-spacex-to-meet-ai-demand',
      ),
      SourceArtifact(
        text: 'Shacknews summary',
        source: 'summary',
        url:
            'https://www.shacknews.com/article/149030/anthropic-spacex-compute-deal',
      ),
    ],
  ),
  ResearchEvent(
    id: 'amazon',
    title: 'Amazon 5 GW agreement',
    date: 'Apr 20, 2026',
    color: 0xff1f6f60,
    summary:
        'A separate compute-capacity event: Anthropic signs for up to 5 GW of AWS capacity and a long-term Trainium commitment.',
    sourceLabel: 'Anthropic',
    artifacts: [],
    url: 'https://www.anthropic.com/news/anthropic-amazon-compute',
  ),
  ResearchEvent(
    id: 'google',
    title: 'Google/Broadcom TPU deal',
    date: 'Apr 6, 2026',
    color: 0xff3169a8,
    summary:
        'A separate TPU-capacity event: Anthropic signs a new agreement with Google and Broadcom for multiple gigawatts of next-generation TPU capacity starting in 2027.',
    sourceLabel: 'Anthropic',
    artifacts: [],
    url: 'https://www.anthropic.com/news/google-broadcom-partnership-compute',
  ),
  ResearchEvent(
    id: 'microsoft',
    title: 'Microsoft/NVIDIA partnership',
    date: 'Nov 18, 2025',
    color: 0xff7560a8,
    summary:
        'A separate strategic partnership event: Anthropic commits to Azure compute and NVIDIA architecture collaboration while Microsoft and NVIDIA invest.',
    sourceLabel: 'Anthropic + Microsoft',
    artifacts: [
      SourceArtifact(
        text: 'Anthropic announcement',
        source: 'official',
        url:
            'https://www.anthropic.com/news/microsoft-nvidia-anthropic-announce-strategic-partnerships',
      ),
      SourceArtifact(
        text: 'Microsoft blog',
        source: 'official',
        url:
            'https://blogs.microsoft.com/blog/2025/11/18/microsoft-nvidia-and-anthropic-announce-strategic-partnerships/',
      ),
    ],
  ),
  ResearchEvent(
    id: 'fluidstack',
    title: 'Fluidstack data centers',
    date: 'Nov 12, 2025',
    color: 0xff98612b,
    summary:
        'A separate infrastructure-investment event: Anthropic announces \$50B in US AI infrastructure with Fluidstack-built data centers in Texas and New York.',
    sourceLabel: 'Anthropic',
    artifacts: [],
    url:
        'https://www.anthropic.com/news/anthropic-invests-50-billion-in-american-ai-infrastructure',
  ),
];

const fixtureBridges = <EventBridge>[
  EventBridge(
    from: 'spacex',
    to: 'amazon',
    label: 'same strategy: capacity portfolio',
  ),
  EventBridge(
    from: 'spacex',
    to: 'google',
    label: 'same strategy: supplier diversity',
  ),
  EventBridge(
    from: 'spacex',
    to: 'microsoft',
    label: 'same strategy: NVIDIA capacity',
  ),
  EventBridge(
    from: 'spacex',
    to: 'fluidstack',
    label: 'same strategy: infrastructure buildout',
  ),
  EventBridge(
    from: 'amazon',
    to: 'google',
    label: 'parallel April 2026 compute deals',
  ),
  EventBridge(
    from: 'microsoft',
    to: 'fluidstack',
    label: 'November 2025 capacity expansion',
  ),
];
