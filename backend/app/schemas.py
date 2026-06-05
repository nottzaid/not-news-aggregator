from __future__ import annotations

from pydantic import BaseModel, Field


class SourceArtifactDto(BaseModel):
    text: str
    source: str
    url: str


class ResearchEventDto(BaseModel):
    id: str
    title: str
    date: str
    color: int
    summary: str
    source_label: str = Field(alias="sourceLabel")
    artifacts: list[SourceArtifactDto] = Field(default_factory=list)
    url: str | None = None

    model_config = {"populate_by_name": True}


class EventBridgeDto(BaseModel):
    from_: str = Field(alias="from")
    to: str
    label: str

    model_config = {"populate_by_name": True}


class SessionMessageDto(BaseModel):
    message: str
