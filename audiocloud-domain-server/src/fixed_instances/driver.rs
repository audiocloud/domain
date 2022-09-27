pub trait InstanceDriverClient {
    fn request(&self, request: InstanceDriverRequest);
}
