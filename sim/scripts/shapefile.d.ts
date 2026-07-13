declare module 'shapefile' {
  export interface ShapefileRecord {
    done: boolean;
    value?: {
      geometry?: {
        type: string;
        coordinates: unknown;
      };
    };
  }

  export interface ShapefileSource {
    read(): Promise<ShapefileRecord>;
  }

  export function open(shp: ArrayBuffer, dbf?: ArrayBuffer): Promise<ShapefileSource>;
}
