"use client";
// Shared touch-capable drag-and-drop primitives for simple column boards
// (move a card between columns). Built on dnd-kit so it works on tablets:
// mouse drags after 5px; touch drags after a ~220ms press-and-hold, which also
// stops the OS long-press/context menu from hijacking the gesture.
import {
  MouseSensor, TouchSensor, useSensor, useSensors, useDraggable, useDroppable,
} from "@dnd-kit/core";
import type { ReactNode, CSSProperties } from "react";

export function useBoardSensors() {
  return useSensors(
    useSensor(MouseSensor, { activationConstraint: { distance: 5 } }),
    useSensor(TouchSensor, { activationConstraint: { delay: 220, tolerance: 8 } }),
  );
}

export function DroppableColumn({ id, className, children }: {
  id: string;
  className?: string;
  children: (isOver: boolean) => ReactNode;   // render-prop exposes hover state
}) {
  const { setNodeRef, isOver } = useDroppable({ id });
  return <div ref={setNodeRef} className={className}>{children(isOver)}</div>;
}

export function DraggableCard({ id, onClick, className, style, children }: {
  id: string;
  onClick?: () => void;
  className?: string;
  style?: CSSProperties;
  children: ReactNode;
}) {
  const { attributes, listeners, setNodeRef, transform, isDragging } = useDraggable({ id });
  const s: CSSProperties = {
    ...style,
    transform: transform ? `translate(${transform.x}px, ${transform.y}px)` : undefined,
    opacity: isDragging ? 0.4 : 1,
    // Not "none": the touch sensor waits for a press-and-hold, so a swipe that starts on a card still
    // scrolls the board (sideways between columns) and the page.
    touchAction: "manipulation",
  };
  return (
    <div ref={setNodeRef} {...attributes} {...listeners} onClick={onClick} style={s} className={className}>
      {children}
    </div>
  );
}
